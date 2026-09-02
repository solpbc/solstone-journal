// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use chrono::{Local, Utc};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use solstone_core_cli::{
    BodyAppleOptions, BodyCommand, BodyOuraCommand, BodyOuraConnectOptions, BodyOuraSyncOptions,
    BodyRebuildOptions, BrainCommand, BrainInspectOptions, BrainPrerequisiteRenewalSessionOptions,
    BrainRefreshExpectArg, BrainRefreshSessionOptions, BrainRuntimeFailureOptions, CHECK_HELP,
    CHECK_USAGE, CONFIG_HELP, CONFIG_USAGE, CONTRACT_BUILD_HELP, CONTRACT_BUILD_USAGE,
    CONTRACT_CHECK_HELP, CONTRACT_CHECK_USAGE, CONTRACT_HELP, CONTRACT_USAGE, CONVEY_HELP,
    CONVEY_USAGE, CORTEX_HELP, CORTEX_USAGE, CogitateCommand, Command, ContractCommand,
    ConveyOptions, ENGAGE_HELP, ENGAGE_USAGE, FACET_CANDIDATES_HELP, FACET_CANDIDATES_USAGE,
    GRAB_HELP, GRAB_USAGE, GenerateCommand, GenerateSessionOptions, GrabCommand, GrabOptions,
    HEALTH_HELP, HEALTH_USAGE, HEARTBEAT_HELP, HEARTBEAT_USAGE, IDENTITY_BRIEFING_HELP,
    IDENTITY_BRIEFING_USAGE, IDENTITY_HEALTH_HELP, IDENTITY_HEALTH_USAGE, IDENTITY_HELP,
    IDENTITY_PARTNER_HELP, IDENTITY_PARTNER_USAGE, IDENTITY_USAGE, INSTALL_MODELS_HELP,
    INSTALL_MODELS_USAGE, INSTALL_PROVIDER_HELP, INSTALL_PROVIDER_USAGE, IndexerCommand,
    IndexerCountsOptions, IndexerFoldEntityEdgesOptions, IndexerOptions, IndexerPrunePathsOptions,
    IndexerPruneStreamOptions, IndexerQueryOptions, IndexerReadOptions, IndexerSearchOptions,
    InstallCommand, JournalBrainOwnerCommand, JournalConfigCommand, JournalConfigCommitOptions,
    JournalConfigExpectArg, JournalConfigReadOptions, JournalPathOptions, LocalCommand, MCP_HELP,
    MCP_USAGE, McpCommand, McpOauthCommand, McpPairingCommand, McpTokenCommand, NAVIGATE_HELP,
    NAVIGATE_USAGE, SCHEDULE_HELP, SCHEDULE_USAGE, SENSE_HELP, SENSE_USAGE, SETTINGS_CONVEY_HELP,
    SETTINGS_CONVEY_USAGE, SETTINGS_HELP, SETTINGS_STATUS_HELP, SETTINGS_USAGE, SPL_HELP,
    SPL_USAGE, START_HELP, START_USAGE, SUPERVISOR_HELP, SUPERVISOR_USAGE, ScheduleOptions,
    SenseOptions, SenseReprocessKind, ServiceAction, ServiceOptions, ServiceParseOutcome,
    SettingsParseError, SpeakerResolveCommand, SplCommand, THINKING_HELP, THINKING_SET_LANE_HELP,
    THINKING_SET_LANE_USAGE, THINKING_USAGE, TOP_HELP, TOP_USAGE, TRANSCRIBE_HELP,
    TRANSCRIBE_USAGE, TRANSFER_USAGE, ThinkingCommand, TranscribeOptions, TransferCommand,
    TransferSendOptions, USAGE, evaluate_args, render_service_diagnostic, version_line,
};
use solstone_core_transcribe::{CliError, CliRunError};
mod brain_owner;
mod check;
mod config;
mod contract;
mod engage;
mod facet_candidates;
mod health;
mod health_logs;
mod heartbeat;
mod identity;
mod import_sources;
mod install_models;
mod install_provider;
mod navigate;
mod service;
mod service_logs;
mod settings;
use solstone_core::supervisor;
#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
use solstone_core::{OAuthStore, OAuthStoreError, TokenStore, TokenStoreError};
use solstone_core_system::lifecycle::{
    ADMISSION_WAIT_TERMINAL_COPY, ADMISSION_WAIT_UNVERIFIABLE_COPY, CoordinatorBootstrap,
    DeclaredParent, HostedServiceKind, HostedServiceParentRuntime, ParentAdmissionFailure,
    ParentLossCoordinator, acknowledge_hosted_child_admission, admit_hosted_service_parent,
    arm_parent_loss_coordinator_termination_guard,
};
use solstone_core_system::process::ProcessInstance;
mod thinking;
use solstone_core_indexer_query::{
    IndexAccessError, Order, SearchRequest, agents, coverage, search, search_counts,
};
use solstone_core_indexer_store::db::{prune_by_paths, prune_chunks_by_stream, reset_index};
use solstone_core_indexer_store::merge::{
    fingerprint_edge_rows, fold_entity_edges_for_recorded_merge,
    rebuild_edges_for_recorded_merge_undo,
};
use solstone_core_indexer_store::scan::{
    RescanFileStatus, rebuild_edges, rescan_file, scan_journal,
};
use solstone_core_journal::{
    ConfigError, HomeError, Source, describe_package_roots_miss, discover_home,
    ensure_journal_dir_with_label, read_config_journal,
    resolve_installation_root_from_executable_dir, resolve_journal_path,
};
#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
use solstone_core_journal_config::{McpEndpointCapability, mcp_endpoint_capability};
use solstone_core_journal_config::{materialized_defaults, read_journal_config};
use solstone_core_journal_config_write::{
    CommitConfigError, ConfigExpectation, LockError, LockOptions, commit_journal_config,
};
mod talent_contract;
mod talent_preview;
mod warm;
const EXIT_USAGE: u8 = 64;
const EXIT_UNAVAILABLE: u8 = 69;
const EXIT_INTERNAL_FAILURE: u8 = 70;
const EXIT_TEMPFAIL: u8 = 75;
const EXIT_DATAERR: u8 = 65;
const EXIT_CANTCREAT: u8 = 73;
const EXIT_IOERR: u8 = 74;
/// `EX_PROTOCOL`: the caller broke a brain-session framing contract.
const EXIT_PROTOCOL: u8 = 76;
/// `EX_CONFIG`: hosted-service parent admission failed before service work.
const EXIT_HOSTED_SERVICE_ADMISSION_REFUSED: u8 = 78;
/// `EX_CONFIG`: the supervisor's own speakers-analyze generation acquisition
/// or installation validation failed before any app process started.
/// Numerically identical to but a distinct case from
/// `EXIT_HOSTED_SERVICE_ADMISSION_REFUSED`, which is the hosted-CHILD
/// admission refusal, not the supervisor's own boot refusal.
const EXIT_SPEAKERS_ANALYZE_GENERATION_REFUSED: u8 = 78;
const MAX_JSON_STDIN_BYTES: usize = 1024 * 1024;
const MAX_LOCAL_STDIN_BYTES: usize = 1024 * 1024;
const SESSION_INPUT_TIMEOUT: Duration = Duration::from_secs(90);
const SESSION_RESULT_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const REFRESH_PROBE_SCHEMA: &str = "solstone.brain.refresh.probe.v1";
const REFRESH_ABANDON_SCHEMA: &str = "solstone.brain.refresh.abandon.v1";
const REFRESH_TERMINAL_SCHEMA: &str = "solstone.brain.refresh.terminal.v1";
const REFRESH_RESULT_SCHEMA: &str = "solstone.brain.refresh.result.v1";
const REFRESH_READY_SCHEMA: &str = "solstone.brain.refresh.ready.v1";
const PREREQUISITE_RENEWAL_PROBE_SCHEMA: &str = "solstone.brain.prerequisite_renewal.probe.v1";
const PREREQUISITE_RENEWAL_ABANDON_SCHEMA: &str = "solstone.brain.prerequisite_renewal.abandon.v1";
const PREREQUISITE_RENEWAL_TERMINAL_SCHEMA: &str =
    "solstone.brain.prerequisite_renewal.terminal.v1";
const PREREQUISITE_RENEWAL_RESULT_SCHEMA: &str = "solstone.brain.prerequisite_renewal.result.v1";
const PREREQUISITE_RENEWAL_READY_SCHEMA: &str = "solstone.brain.prerequisite_renewal.ready.v1";
const MAX_LOCAL_GENERATE_STDIN_BYTES: usize = 64 * 1024 * 1024;
const ZERO_EDGE_HINT: &str = "Zero edges indexed: edges are talent-derived, and the --rescan-full edge phase remains modification-time incremental — run journal indexer --rebuild-edges to force full edge re-extraction.";
struct JournalPathLine {
    label: &'static str,
    path: PathBuf,
}

enum JournalPathError {
    Config(ConfigError),
    Home(HomeError),
    Create(solstone_core_journal::EnsureJournalDirError),
}

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

fn resolve_declared_parent(
    hosted_parent: bool,
) -> Result<Option<DeclaredParent>, ParentAdmissionFailure> {
    if hosted_parent {
        DeclaredParent::capture_current().map(Some)
    } else {
        Ok(None)
    }
}

/// Gate every supervisor-spawned service before it can bind or publish any
/// readiness artifact. Unhosted commands receive `None` and retain their
/// existing service behavior unchanged.
fn run_hosted_service<F>(journal: &Path, kind: HostedServiceKind, run: F) -> ExitCode
where
    F: FnOnce(Option<Arc<HostedServiceParentRuntime>>) -> ExitCode,
{
    match admit_hosted_service_parent(journal, kind) {
        Ok(parent) => run(parent.map(Arc::new)),
        Err(error) => {
            eprintln!(
                "hosted {} service admission refused: {error}",
                kind_name(kind)
            );
            ExitCode::from(EXIT_HOSTED_SERVICE_ADMISSION_REFUSED)
        }
    }
}

const fn kind_name(kind: HostedServiceKind) -> &'static str {
    match kind {
        HostedServiceKind::Convey => "convey",
        HostedServiceKind::Sense => "sense",
        HostedServiceKind::Cortex => "cortex",
        HostedServiceKind::Spl => "spl",
        HostedServiceKind::Mcp => "mcp",
    }
}

fn run_supervisor(options: solstone_core_cli::SupervisorOptions) -> ExitCode {
    let parent = match resolve_declared_parent(options.hosted_parent) {
        Ok(parent) => parent,
        Err(error) => {
            return render_supervisor_host_outcome(supervisor::SupervisorHostOutcome::Refused {
                reason: supervisor::SupervisorBootRefusal::ParentLiveness(error),
            });
        }
    };
    let journal = match resolve_journal_config_path(options.journal_override.clone()) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("supervisor runtime unavailable: {error}");
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    render_supervisor_host_outcome(
        runtime.block_on(supervisor::run_hosted(&journal, options, parent)),
    )
}

fn render_supervisor_host_outcome(outcome: supervisor::SupervisorHostOutcome) -> ExitCode {
    match outcome {
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::SyncScan(copy),
        } => {
            eprintln!("{copy}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::InstallationBinding(refusal),
        } => {
            eprintln!("{refusal}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::AdmissionWaitTerminal,
        } => {
            eprintln!("{ADMISSION_WAIT_TERMINAL_COPY}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::AdmissionWaitUnverifiable,
        } => {
            eprintln!("{ADMISSION_WAIT_UNVERIFIABLE_COPY}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::SpeakersAnalyzeGeneration(message),
        } => {
            eprintln!("{message}");
            ExitCode::from(EXIT_SPEAKERS_ANALYZE_GENERATION_REFUSED)
        }
        supervisor::SupervisorHostOutcome::Refused { reason } => {
            eprintln!("supervisor failed to boot: {reason:?}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::LifecycleShutdownFailed {
            cause,
            readiness,
            self_heartbeat,
            identity,
        } => {
            eprintln!(
                "supervisor lifecycle shutdown failed: cause={cause:?}, readiness={readiness:?}, self_heartbeat={self_heartbeat:?}, identity={identity:?}"
            );
            ExitCode::from(EXIT_INTERNAL_FAILURE)
        }
        supervisor::SupervisorHostOutcome::ParentLost { .. } => ExitCode::from(EXIT_TEMPFAIL),
        supervisor::SupervisorHostOutcome::OrderlyShutdown { cause }
        | supervisor::SupervisorHostOutcome::ForcedShutdownAfterGraceTimeout { cause, .. } => {
            ExitCode::from(exit_code_for_shutdown_cause(cause))
        }
    }
}

fn exit_code_for_shutdown_cause(cause: supervisor::ShutdownCause) -> u8 {
    match cause {
        supervisor::ShutdownCause::Sync(supervisor::SyncFailureKind::Conflict) => 2,
        supervisor::ShutdownCause::Sync(
            supervisor::SyncFailureKind::RenewalFailure
            | supervisor::SyncFailureKind::CompleteScanFailure
            | supervisor::SyncFailureKind::RetainedObservationFailure
            | supervisor::SyncFailureKind::StaleHeartbeatCollectionFailure,
        ) => EXIT_TEMPFAIL,
        supervisor::ShutdownCause::ParentLost(_) => EXIT_TEMPFAIL,
        supervisor::ShutdownCause::Signal(_) => 0,
    }
}

fn main() -> ExitCode {
    install_logger();
    let args: Vec<_> = env::args_os().skip(1).collect();
    match evaluate_args(&args) {
        Ok(Command::Version) => {
            print!("{}", version_line(env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Ok(Command::Assets) => run_assets(),
        Ok(Command::Warm { json }) => {
            if warm::run(json) {
                ExitCode::from(EXIT_UNAVAILABLE)
            } else {
                ExitCode::SUCCESS
            }
        }
        Ok(Command::Check { json }) => ExitCode::from(check::run(json)),
        Ok(Command::Config(command)) => config::run(command),
        Ok(Command::ConfigHelp) => {
            print!("{CONFIG_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ConfigUsage) => render_usage_error(CONFIG_USAGE, "journal config"),
        Ok(Command::CheckUsage) => render_usage_error(CHECK_USAGE, "journal check"),
        Ok(Command::CheckHelp) => {
            print!("{CHECK_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Doctor(args)) => run_doctor(args),
        Ok(Command::DoctorHelp) => {
            print!("{}", solstone_core_doctor::args::HELP);
            ExitCode::SUCCESS
        }
        Ok(Command::DoctorUsage(error)) => {
            eprint!("{}", solstone_core_doctor::args::USAGE);
            eprintln!("journal doctor: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::Setup(args)) => run_setup(args),
        Ok(Command::SetupHelp) => {
            print!("{}", solstone_core_setup::args::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::SetupUsage(error)) => {
            eprint!("{}", solstone_core_setup::args::USAGE);
            eprintln!("journal setup: error: {}", error.0);
            ExitCode::from(2)
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
        Ok(Command::SpeakerTranscriptWrite) => run_speaker_transcript_write(),
        Ok(Command::SpeakerResolve(command)) => run_speaker_resolve(command),
        Ok(Command::Local(command)) => run_local(command),
        Ok(Command::Generate(command)) => run_generate(command),
        Ok(Command::Cogitate(command)) => run_cogitate(command),
        Ok(Command::Brain(command)) => run_brain(command),
        Ok(Command::JournalBrainOwner(command)) => match command {
            JournalBrainOwnerCommand::Help => {
                print!("{}", solstone_core_cli::BRAIN_OWNER_HELP);
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::StatusHelp => {
                print!("{}", solstone_core_cli::BRAIN_STATUS_HELP);
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::RefreshHelp => {
                print!("{}", solstone_core_cli::BRAIN_REFRESH_HELP);
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::RenewPrerequisitesHelp => {
                print!("{}", solstone_core_cli::BRAIN_RENEW_PREREQUISITES_HELP);
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::Bare => {
                print!("{}", solstone_core_cli::BRAIN_OWNER_HELP);
                ExitCode::from(2)
            }
            JournalBrainOwnerCommand::Usage => {
                render_usage_error(solstone_core_cli::BRAIN_OWNER_USAGE, "journal brain")
            }
            command => brain_owner::run(command),
        },
        Ok(Command::Body(command)) => run_body(command),
        Ok(Command::Transfer(command)) => run_transfer(command),
        Ok(Command::RetiredMover(message)) => run_retired_mover(message),
        Ok(Command::Transcribe(options)) => run_transcribe(options),
        Ok(Command::Think(args)) => run_storage_ops_verb("think", args, |arguments, journal| {
            // Only the whole-day lifecycle (the sole caller of `sense_batch`)
            // needs a speakers-analyze generation; narrower modes (`--segment`,
            // `--flush`, `--weekly`, `--cadence`, `--activity`, `--updated`,
            // `--segments`, `--dry-run`) never reach it and must not be forced
            // to contend for a lease they don't need.
            if !solstone_core_think_cli::requires_daily_lifecycle(arguments) {
                let run = solstone_core_think_cli::run_cli(arguments, journal, &BTreeMap::new());
                return (run.stdout, run.stderr, run.exit_code);
            }
            let generation = match solstone_core_transcribe::enter_speakers_analyze_generation(
                journal,
                solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Think,
            ) {
                Ok(generation) => generation,
                Err(error) => {
                    return (
                        String::new(),
                        error.message().unwrap_or_default().to_owned(),
                        error.exit_code(),
                    );
                }
            };
            let sense_child_environment = generation.inheritance_environment();
            let run =
                solstone_core_think_cli::run_cli(arguments, journal, &sense_child_environment);
            drop(generation);
            (run.stdout, run.stderr, run.exit_code)
        }),
        Ok(Command::ThinkingHelp) => {
            print!("{THINKING_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ThinkingUsage) => render_usage_error(THINKING_USAGE, "journal thinking"),
        Ok(Command::Thinking(ThinkingCommand::SetLaneHelp)) => {
            print!("{THINKING_SET_LANE_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::SetLaneUsage)) => {
            render_usage_error(THINKING_SET_LANE_USAGE, "journal thinking set-lane")
        }
        Ok(Command::Thinking(ThinkingCommand::SetLane(options))) => thinking::run(options),
        Ok(Command::Streams(args)) => run_streams(args),
        Ok(Command::Importer(args)) => run_importer(args),
        Ok(Command::Segment(args)) => run_segment(args),
        Ok(Command::Backup(args)) => run_backup(args),
        Ok(Command::Maintenance(args)) => run_maintenance(args),
        Ok(Command::TalentWorker(args)) => run_talent_worker(args),
        Ok(Command::ParentLossCoordinator(args)) => run_parent_loss_coordinator(args),
        Ok(Command::Reprocess(args)) => run_reprocess(args),
        Ok(Command::JournalStats(args)) => run_journal_stats(args),
        Ok(Command::Talent(args)) => run_talent(args),
        Ok(Command::Backfill(args)) => run_backfill(args),
        Ok(Command::FacetCandidates) => run_facet_candidates(),
        Ok(Command::InstallModels(options)) => install_models::run(options),
        Ok(Command::InstallModelsUsage) => {
            render_usage_error(INSTALL_MODELS_USAGE, "journal install-models")
        }
        Ok(Command::InstallModelsHelp) => {
            print!("{INSTALL_MODELS_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::InstallProvider(options)) => install_provider::run(options),
        Ok(Command::InstallProviderUsage) => {
            render_usage_error(INSTALL_PROVIDER_USAGE, "journal install-provider")
        }
        Ok(Command::InstallProviderHelp) => {
            print!("{INSTALL_PROVIDER_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Convey(options)) => run_convey(options),
        Ok(Command::ConveyHelp) => {
            print!("{CONVEY_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ConveyUsage(error)) => {
            eprint!("{CONVEY_USAGE}");
            eprintln!("journal convey: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::Schedule(options)) => run_schedule(options),
        Ok(Command::ScheduleHelp) => {
            print!("{SCHEDULE_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ScheduleUsage(error)) => {
            eprint!("{SCHEDULE_USAGE}");
            eprintln!("journal schedule: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::Grab(command)) => run_grab(command),
        Ok(Command::Spl(command)) => run_spl_process(command),
        Ok(Command::SplUsage(error)) => {
            eprint!("{SPL_USAGE}");
            eprintln!("journal spl: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::SplHelp) => {
            print!("{SPL_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Mcp(command)) => run_mcp_process(command),
        Ok(Command::McpUsage(error)) => {
            eprint!("{MCP_USAGE}");
            eprintln!("journal mcp: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::McpHelp) => {
            print!("{MCP_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Sense(options)) => run_sense(options),
        Ok(Command::SenseUsage) => render_usage_error(SENSE_USAGE, "journal sense"),
        Ok(Command::SenseHelp) => {
            print!("{SENSE_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Cortex(options)) => run_cortex_service(options),
        Ok(Command::CortexUsage(error)) => {
            eprint!("{CORTEX_USAGE}");
            eprintln!("journal cortex: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::CortexHelp) => {
            print!("{CORTEX_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Supervisor(options)) => run_supervisor(options),
        Ok(Command::SupervisorUsage) => render_usage_error(SUPERVISOR_USAGE, "journal supervisor"),
        Ok(Command::SupervisorInvalid(error)) => {
            eprint!("{SUPERVISOR_USAGE}");
            eprintln!("journal supervisor: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::SupervisorHelp) => {
            print!("{SUPERVISOR_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::StartUsage) => render_usage_error(START_USAGE, "journal start"),
        Ok(Command::StartInvalid(error)) => {
            eprint!("{START_USAGE}");
            eprintln!("journal start: error: {}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::StartHelp) => {
            print!("{START_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::SupervisorLifecycleRedirect(verb)) => {
            eprintln!(
                "journal supervisor is the server-launch command (takes a port). \
                 For lifecycle, use: journal service <verb>. Did you mean: journal service {verb} ?"
            );
            ExitCode::from(2)
        }
        Ok(Command::Health { verbose, debug }) => health::run(verbose, debug),
        Ok(Command::HealthUsage) => render_usage_error(HEALTH_USAGE, "journal health"),
        Ok(Command::HealthHelp) => {
            print!("{HEALTH_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Top { verbose, debug }) => match solstone_core_top::run(verbose, debug) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("journal top: {error}");
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        Ok(Command::TopUsage) => render_usage_error(TOP_USAGE, "journal top"),
        Ok(Command::TopHelp) => {
            print!("{TOP_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::HealthLogs(args)) => health_logs::run(args),
        Ok(Command::HealthLogsUsage(args)) => health_logs::usage(args),
        Ok(Command::HealthLogsHelp(args)) => health_logs::help(args),
        Ok(Command::Heartbeat { force }) => match resolve_process_journal_path() {
            Ok(resolved) => heartbeat::run(&resolved.path, force),
            Err(error) => print_journal_error(error),
        },
        Ok(Command::HeartbeatUsage) => render_usage_error(HEARTBEAT_USAGE, "journal heartbeat"),
        Ok(Command::HeartbeatHelp) => {
            print!("{HEARTBEAT_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Engage(options)) => match resolve_process_journal_path() {
            Ok(resolved) => engage::run(&resolved.path, options),
            Err(error) => print_journal_error(error),
        },
        Ok(Command::EngageUsage) => render_usage_error(ENGAGE_USAGE, "journal engage"),
        Ok(Command::EngageHelp) => {
            print!("{ENGAGE_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Service(outcome)) => match outcome {
            ServiceParseOutcome::Dispatch(ServiceAction::Logs { follow }) => {
                service_logs::run(solstone_core_cli::ServiceLogsArgs { follow })
            }
            ServiceParseOutcome::Dispatch(action) => service::run(action),
            ServiceParseOutcome::Exit {
                code,
                stdout,
                stderr,
            } => {
                if let Some(stdout) = stdout {
                    print!("{stdout}");
                }
                if let Some(stderr) = stderr {
                    eprint!("{}", render_service_diagnostic(&stderr));
                }
                ExitCode::from(code)
            }
        },
        Ok(Command::Navigate { path }) => navigate::run(path),
        Ok(Command::NavigateHelp) => {
            print!("{NAVIGATE_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::NavigateUsage) => {
            eprint!("{NAVIGATE_USAGE}");
            eprintln!("journal navigate: error: invalid arguments");
            ExitCode::from(2)
        }
        Ok(Command::NavigateFacetRetired(option)) => {
            eprint!("{NAVIGATE_USAGE}");
            eprintln!(
                "journal navigate: error: {option} is no longer supported. Put facet selection in the destination URL; for example, /app/entities?facet=work."
            );
            ExitCode::from(2)
        }
        Ok(Command::Identity(command)) => identity::run(command),
        Ok(Command::Contract(ContractCommand::Build { check, root })) => {
            contract::run_build(check, root)
        }
        Ok(Command::Contract(ContractCommand::Check { journals, root })) => {
            contract::run_check(journals, root)
        }
        Ok(Command::ContractHelp) => {
            print!("{CONTRACT_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ContractUsage) => contract_usage(CONTRACT_USAGE, "journal contract"),
        Ok(Command::ContractBuildHelp) => {
            print!("{CONTRACT_BUILD_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ContractBuildUsage) => {
            contract_usage(CONTRACT_BUILD_USAGE, "journal contract build")
        }
        Ok(Command::ContractCheckHelp) => {
            print!("{CONTRACT_CHECK_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::ContractCheckUsage) => {
            contract_usage(CONTRACT_CHECK_USAGE, "journal contract check")
        }
        Ok(Command::IdentityHelp) => {
            print!("{IDENTITY_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::IdentityUsage) => render_usage_error(IDENTITY_USAGE, "journal identity"),
        Ok(Command::IdentityUnknownCommand(command)) => identity_unknown_command(&command),
        Ok(Command::IdentityPartnerHelp) => {
            print!("{IDENTITY_PARTNER_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::IdentityPartnerUsage) => {
            render_usage_error(IDENTITY_PARTNER_USAGE, "journal identity partner")
        }
        Ok(Command::IdentityHealthHelp) => {
            print!("{IDENTITY_HEALTH_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::IdentityHealthUsage) => {
            render_usage_error(IDENTITY_HEALTH_USAGE, "journal identity health")
        }
        Ok(Command::IdentityBriefingHelp) => {
            print!("{IDENTITY_BRIEFING_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::IdentityBriefingUsage) => {
            render_usage_error(IDENTITY_BRIEFING_USAGE, "journal identity briefing")
        }
        Ok(Command::Settings(command)) => settings::run(command),
        Ok(Command::SettingsHelp) => {
            print!("{SETTINGS_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::SettingsConveyHelp) => {
            print!("{SETTINGS_CONVEY_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::SettingsStatusHelp) => {
            print!("{SETTINGS_STATUS_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::SettingsParseError(error)) => settings_parse_error(error),
        Ok(Command::TransferHelp(text)) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Command::TranscribeHelp) => {
            print!("{TRANSCRIBE_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::FacetCandidatesHelp) => {
            print!("{FACET_CANDIDATES_HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::TransferUsage) => {
            eprint!("{TRANSFER_USAGE}");
            eprintln!("journal transfer: error: invalid arguments");
            ExitCode::from(2)
        }
        Ok(Command::FacetCandidatesUsage) => {
            eprint!("{FACET_CANDIDATES_USAGE}");
            eprintln!("journal facet-candidates: error: invalid arguments");
            ExitCode::from(2)
        }
        Err(_) => {
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn render_usage_error(usage: &str, command: &str) -> ExitCode {
    // `UsageError` is unit-shaped, so these parsers deliberately use the house
    // message instead of argparse's per-failure detail. Transcribe preserves detail
    // through its separate crate's `CliError::Usage { message }`.
    eprint!("{usage}");
    eprintln!("{command}: error: invalid arguments");
    ExitCode::from(2)
}

fn contract_usage(usage: &str, command: &str) -> ExitCode {
    eprint!("{usage}");
    eprintln!("{command}: error: invalid arguments");
    ExitCode::from(2)
}

fn identity_unknown_command(command: &str) -> ExitCode {
    eprint!("{IDENTITY_USAGE}");
    eprintln!("journal identity: error: invalid choice: '{command}'");
    ExitCode::from(2)
}

fn settings_parse_error(error: SettingsParseError) -> ExitCode {
    match error {
        SettingsParseError::InvalidSection(value) => {
            eprint!("{SETTINGS_USAGE}");
            eprintln!(
                "journal settings: error: argument section: invalid choice: '{value}' (choose from convey)"
            );
        }
        SettingsParseError::InvalidConveyCommand(value) => {
            eprint!("{SETTINGS_CONVEY_USAGE}");
            eprintln!(
                "journal settings convey: error: argument convey_command: invalid choice: '{value}' (choose from status)"
            );
        }
        SettingsParseError::UnrecognizedArgument(value) => {
            eprint!("{SETTINGS_USAGE}");
            eprintln!("journal settings: error: unrecognized arguments: {value}");
        }
    }
    ExitCode::from(2)
}

struct StderrGrabDiagnostics {
    show_warnings: bool,
}

impl solstone_core_grab::GrabDiagnostics for StderrGrabDiagnostics {
    fn malformed_jsonl(&mut self, path: &Path, line: usize, error: &str) {
        if self.show_warnings {
            eprintln!(
                "WARNING:solstone.observe.utils:Invalid JSON at line {line} in {}: {error}",
                path.display()
            );
        }
    }

    fn read_error(&mut self, path: &Path, error: &io::Error) {
        eprintln!(
            "ERROR:solstone.observe.utils:Error reading {}: {error}",
            path.display()
        );
    }
}

fn run_grab(command: GrabCommand) -> ExitCode {
    match command {
        GrabCommand::Help => {
            print!("{GRAB_HELP}");
            ExitCode::SUCCESS
        }
        GrabCommand::ParseError(message) => {
            eprint!("{GRAB_USAGE}");
            // argparse prefixes the verb; the bare `error:` named no command.
            eprintln!("journal grab: error: {message}");
            ExitCode::from(2)
        }
        GrabCommand::Run(options) => run_grab_request(options),
    }
}

fn run_grab_request(options: GrabOptions) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let json = options.json;
    let mut diagnostics = StderrGrabDiagnostics {
        show_warnings: options.verbose || options.debug,
    };
    let request = solstone_core_grab::GrabRequest {
        tokens: options.tokens,
        out: options.out.map(PathBuf::from),
        force: options.force,
    };
    match solstone_core_grab::run(&journal.path, request, &mut diagnostics) {
        Ok(output) => {
            if json {
                match serde_json::to_string_pretty(&output.payload) {
                    Ok(payload) => println!("{payload}"),
                    Err(error) => {
                        eprintln!("{error}");
                        return ExitCode::from(1);
                    }
                }
            } else {
                print!("{}", output.human);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            match error.class() {
                solstone_core_grab::GrabFailureClass::Usage => ExitCode::from(2),
                solstone_core_grab::GrabFailureClass::Runtime => ExitCode::from(1),
            }
        }
    }
}

fn run_transcribe(options: TranscribeOptions) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
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
    match solstone_core_transcribe::run_cli(options.arguments, &journal.path, &mut on_day) {
        Ok(result) => {
            if let Some(summary) = result.summary {
                println!("{summary}");
            }
            if let Some(stderr) = result.stderr {
                eprint!("{stderr}");
            }
            ExitCode::from(result.exit_code as u8)
        }
        Err(CliRunError::Cli(CliError::Usage { message })) => {
            eprint!("{TRANSCRIBE_USAGE}");
            eprintln!("journal transcribe: error: {message}");
            ExitCode::from(2)
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

fn require_utf8_argv(verb: &str, args: &[OsString]) -> Result<Vec<String>, ExitCode> {
    match args
        .iter()
        .map(|argument| argument.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    {
        Some(arguments) => Ok(arguments),
        None => {
            eprintln!("native solstone-core {verb} failed: arguments are not valid UTF-8");
            Err(ExitCode::from(EXIT_TEMPFAIL))
        }
    }
}

fn run_storage_ops_verb(
    verb: &str,
    args: Vec<OsString>,
    call: impl FnOnce(&[String], &Path) -> (String, String, i32),
) -> ExitCode {
    let arguments = match require_utf8_argv(verb, &args) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    let is_help = arguments
        .iter()
        .any(|argument| argument == "-h" || argument == "--help");
    let journal_path = if is_help {
        PathBuf::new()
    } else {
        match resolve_process_journal_path() {
            Ok(journal) => journal.path,
            Err(error) => {
                eprint_journal_path_error(error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    };
    let (stdout, stderr, exit_code) = call(&arguments, &journal_path);
    print!("{stdout}");
    eprint!("{stderr}");
    ExitCode::from(exit_code as u8)
}

fn run_streams(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("streams", args, |arguments, journal| {
        let run = solstone_core_streams_cli::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_importer(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("importer", args, |arguments, journal| {
        let run = match solstone_core_import_host::cli_argv::run_cli(arguments, journal) {
            solstone_core_import_host::cli_argv::CliOutcome::Rendered(run) => run,
            solstone_core_import_host::cli_argv::CliOutcome::Registry(dispatch) => {
                import_sources::run(dispatch, journal)
            }
        };
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_segment(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("segment", args, |arguments, journal| {
        let run = solstone_core_segment_cli::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_backup(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("backup", args, |arguments, journal| {
        let run = solstone_core_backup_cli::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_maintenance(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("maintenance", args, |arguments, journal| {
        let run = solstone_core_maintenance::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_talent_worker(args: Vec<OsString>) -> ExitCode {
    let arguments = match require_utf8_argv("talent worker", &args) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };
    if let Err(error) = acknowledge_hosted_child_admission(&journal) {
        eprintln!("talent worker parent-loss admission failed: {error}");
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    solstone_core_talent_runtime::run_worker(&arguments, &journal)
}

fn run_parent_loss_coordinator(args: Vec<OsString>) -> ExitCode {
    if let Err(error) = arm_parent_loss_coordinator_termination_guard() {
        eprintln!("parent-loss coordinator: could not block group-termination signal: {error}");
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    let mut supervisor = None;
    let mut enabled = None;
    let mut supervisor_heartbeat_filename = None;
    let mut arguments = args.iter().filter_map(|argument| argument.to_str());
    while let Some(argument) = arguments.next() {
        match argument {
            "--supervisor-json" => {
                supervisor = arguments
                    .next()
                    .and_then(|value| serde_json::from_str::<ProcessInstance>(value).ok());
            }
            "--enabled-json" => {
                enabled = arguments
                    .next()
                    .and_then(|value| serde_json::from_str::<Vec<HostedServiceKind>>(value).ok());
            }
            "--supervisor-heartbeat" => {
                supervisor_heartbeat_filename = arguments.next().map(ToOwned::to_owned);
            }
            _ => {}
        }
    }
    let (Some(supervisor), Some(enabled), Some(supervisor_heartbeat_filename)) =
        (supervisor, enabled, supervisor_heartbeat_filename)
    else {
        eprintln!("parent-loss coordinator: invalid bootstrap arguments");
        return ExitCode::from(2);
    };
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };
    let mut capability = Vec::new();
    if let Err(error) = std::io::Read::read_to_end(&mut std::io::stdin(), &mut capability) {
        eprintln!("parent-loss coordinator: could not read bootstrap capability: {error}");
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    if capability.len() < 32 {
        eprintln!("parent-loss coordinator: missing private bootstrap capability");
        return ExitCode::from(2);
    }
    let (coordinator, _) = match ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
        journal,
        supervisor,
        enabled,
        supervisor_heartbeat_filename,
        capability,
    }) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("parent-loss coordinator: bootstrap failed: {error}");
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    match coordinator.run() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("parent-loss coordinator: lifecycle failed: {error}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
    }
}

fn run_reprocess(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("reprocess", args, |arguments, journal| {
        let run = solstone_core_reprocess_cli::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_journal_stats(args: Vec<OsString>) -> ExitCode {
    let is_help = args
        .iter()
        .any(|argument| argument == OsStr::new("-h") || argument == OsStr::new("--help"));
    let (journal_root, system_talent_root, apps_root) = if is_help {
        (PathBuf::new(), PathBuf::new(), PathBuf::new())
    } else {
        let journal_root = match resolve_process_journal_path() {
            Ok(journal) => journal.path,
            Err(error) => {
                eprint_journal_path_error(error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let executable_dir = env::current_exe()
            .ok()
            .and_then(|executable| executable.parent().map(Path::to_path_buf));
        let Some((system_talent_root, apps_root)) = discover_package_roots() else {
            eprintln!(
                "journal-stats failed: {}",
                describe_package_roots_miss(executable_dir.as_deref().unwrap_or(Path::new("")))
            );
            return ExitCode::from(EXIT_TEMPFAIL);
        };
        (journal_root, system_talent_root, apps_root)
    };
    let backlog_reader = solstone_core_journal_stats_cli::FilesystemBacklogViewReader;
    let document_writer = solstone_core_journal_stats_cli::FilesystemDocumentWriter;
    let run = solstone_core_journal_stats_cli::run_cli(
        &args,
        &journal_root,
        Utc::now(),
        &system_talent_root,
        &apps_root,
        &backlog_reader,
        &document_writer,
    );
    print!("{}", run.stdout);
    eprint!("{}", run.stderr);
    ExitCode::from(run.exit_code as u8)
}

fn run_talent(args: Vec<OsString>) -> ExitCode {
    let is_help = args
        .iter()
        .any(|argument| argument == OsStr::new("-h") || argument == OsStr::new("--help"));
    let run = if is_help {
        talent_preview::run_talent_cli(
            &args,
            Path::new(""),
            Path::new(""),
            Path::new(""),
            SystemTime::UNIX_EPOCH,
        )
    } else {
        let journal_root = match resolve_process_journal_path() {
            Ok(journal) => journal.path,
            Err(error) => {
                eprint_journal_path_error(error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let executable = match env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                eprintln!("talent failed: could not inspect current executable: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let Some(executable_dir) = executable.parent() else {
            eprintln!("talent failed: could not locate installed solstone package");
            return ExitCode::from(EXIT_TEMPFAIL);
        };
        let Some(root) = resolve_installation_root_from_executable_dir(executable_dir) else {
            eprintln!("talent failed: could not locate installed solstone package");
            return ExitCode::from(EXIT_TEMPFAIL);
        };
        talent_preview::run_talent_cli(
            &args,
            &root.join("solstone/talent"),
            &root.join("solstone/apps"),
            &journal_root,
            SystemTime::now(),
        )
    };
    print!("{}", run.stdout);
    eprint!("{}", run.stderr);
    ExitCode::from(run.exit_code as u8)
}

fn run_backfill(args: Vec<OsString>) -> ExitCode {
    let is_help = args
        .iter()
        .any(|argument| argument == OsStr::new("-h") || argument == OsStr::new("--help"));
    let journal = if is_help {
        PathBuf::new()
    } else {
        match resolve_process_journal_path() {
            Ok(journal) => journal.path,
            Err(error) => {
                eprint_journal_path_error(error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    };
    let writer = solstone_core_backfill_cli::AtomicWriter;
    let stdout = io::stdout();
    let stderr = io::stderr();
    let exit_code = solstone_core_backfill_cli::run(
        &args,
        &journal,
        Utc::now(),
        &writer,
        &mut stdout.lock(),
        &mut stderr.lock(),
    );
    ExitCode::from(exit_code as u8)
}

fn run_facet_candidates() -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    facet_candidates::run(&journal.path)
}

fn run_retired_mover(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(2)
}

fn run_transfer(command: TransferCommand) -> ExitCode {
    match command {
        TransferCommand::RetiredMover(message) => run_retired_mover(message),
        TransferCommand::Send(options) => run_transfer_send(options),
    }
}

fn run_transfer_send(options: TransferSendOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match solstone_core_transfer::peer_export(
        &journal,
        solstone_core_transfer::PeerExportRequest {
            to: options.to,
            only: options.only,
            day: options.day,
            dry_run: options.dry_run,
        },
    ) {
        Ok(report) => {
            print_peer_export_summary(&report);
            if report.any_failed {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            let exit = match &error {
                solstone_core_transfer::TransferError::Bridge(_)
                | solstone_core_transfer::TransferError::Transport(_) => 1,
                _ => 2,
            };
            let message = match error {
                solstone_core_transfer::TransferError::InvalidExportAreas => {
                    "--only must contain one or more of: config, entities, facets, imports, segments"
                        .to_string()
                }
                _ => error.to_string(),
            };
            eprintln!("journal transfer send: error: {message}");
            ExitCode::from(exit)
        }
    }
}

fn print_peer_export_summary(report: &solstone_core_transfer::PeerExportReport) {
    println!("\n--- Export Summary ---");
    for result in &report.results {
        if let Some(error) = &result.error {
            println!("  {}: FAILED ({error})", result.area);
            continue;
        }
        let mut parts = Vec::new();
        if result.sent != 0 {
            parts.push(format!("{} sent", result.sent));
        }
        if result.skipped != 0 {
            parts.push(format!("{} skipped", result.skipped));
        }
        if result.staged != 0 {
            parts.push(format!("{} staged", result.staged));
        }
        if result.failed != 0 {
            parts.push(format!("{} failed", result.failed));
        }
        if !result.errors.is_empty() {
            parts.push(format!("{} error(s)", result.errors.len()));
        }
        if parts.is_empty() {
            parts.push("nothing to send".to_string());
        }
        println!("  {}: {}", result.area, parts.join(", "));
    }
}

fn run_convey(options: ConveyOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.clone();
    run_hosted_service(&journal, HostedServiceKind::Convey, move |parent| {
        match solstone_core_convey_shell::run_convey_with_hosted_parent(
            service_journal,
            options.port,
            parent,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(EXIT_TEMPFAIL)
            }
        }
    })
}

fn run_schedule(_options: ScheduleOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(None) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let wall = chrono::Local::now();
    let report = solstone_core_system::schedule::build_schedule_report(
        journal.join("config/schedules.json"),
        journal.join("health/scheduler.json"),
        solstone_core_system::schedule::ScheduleNow {
            local: wall.naive_local(),
            unix_millis: wall.timestamp_millis(),
        },
    );
    print!("{}", report.render());
    for diagnostic in report.diagnostics {
        eprintln!("{diagnostic}");
    }
    ExitCode::from(report.exit_code)
}

fn run_speaker_resolve(command: SpeakerResolveCommand) -> ExitCode {
    let limit = if matches!(command, SpeakerResolveCommand::AccumulateVoiceprints) {
        MAX_LOCAL_GENERATE_STDIN_BYTES
    } else {
        MAX_JSON_STDIN_BYTES
    };
    let value = match read_bounded_json_value(limit) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("speaker-resolve failed: stdin was not a valid JSON request");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let result = match command {
        SpeakerResolveCommand::AccumulateVoiceprints => parse_accumulation_request(value)
            .and_then(|request| {
                solstone_core_speaker_resolve::voiceprint_accumulation::accumulate_voiceprints(
                    &request,
                )
                .map_err(|error| error.to_string())
            })
            .and_then(|outcome| match outcome {
                solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationOutcome::IdentityInvalid {
                    entity_reports,
                } => Ok(json!({
                    "status":"speaker_owner_identity_invalid",
                    "entity_reports":entity_reports,
                })),
                outcome => serde_json::to_value(outcome).map_err(|error| error.to_string()),
            }),
        SpeakerResolveCommand::WriteOwnerCentroid => write_owner_centroid_request(value),
        SpeakerResolveCommand::RebuildOwnerCentroid => rebuild_owner_centroid_request(value),
        SpeakerResolveCommand::WriteOwnerCandidate => write_owner_candidate_request(value),
        SpeakerResolveCommand::ReadOwnerCandidate => read_owner_candidate_request(value),
        SpeakerResolveCommand::ScreenOwnerContamination => {
            screen_owner_contamination_request(value)
        }
        SpeakerResolveCommand::ClearOwnerCandidate => clear_owner_candidate_request(value),
        SpeakerResolveCommand::WriteVoiceprint => write_voiceprint_request(value),
        SpeakerResolveCommand::RemoveVoiceprint => remove_voiceprint_request(value),
        SpeakerResolveCommand::BackfillVoiceprintLastSeen => {
            backfill_voiceprint_last_seen_request(value)
        }
        SpeakerResolveCommand::WriteStubLabels => write_stub_labels_request(value),
        SpeakerResolveCommand::WriteFullLabels => write_full_labels_request(value),
        SpeakerResolveCommand::PatchLabels => patch_labels_request(value),
        SpeakerResolveCommand::RestoreLabelRows => restore_label_rows_request(value),
        SpeakerResolveCommand::AppendCorrection => append_correction_request(value),
        SpeakerResolveCommand::WipeSpeakerArtifacts => wipe_speaker_artifacts_request(value),
        SpeakerResolveCommand::Identify => identify_request(value),
        SpeakerResolveCommand::UndoIdentify => undo_identify_request(value),
        SpeakerResolveCommand::BootstrapVoiceprints => bootstrap_request(value, false),
        SpeakerResolveCommand::SeedFromImports => bootstrap_request(value, true),
        SpeakerResolveCommand::MergeNames => merge_names_request(value),
        SpeakerResolveCommand::Backfill => backfill_request(value),
        SpeakerResolveCommand::BackfillStatus => backfill_status_request(value),
    };
    match result {
        Ok(output) => {
            if serde_json::to_writer(io::stdout().lock(), &output).is_ok() {
                println!();
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_IOERR)
            }
        }
        Err(error) => {
            let exit = speaker_resolve_error_exit(&error);
            eprintln!(
                "{}",
                json!({
                    "error":"speaker_resolve_failed",
                    "detail":error,
                    "exit_code":exit,
                })
            );
            ExitCode::from(exit)
        }
    }
}

fn speaker_resolve_error_exit(error: &str) -> u8 {
    if error.contains("entity not found") || error.contains("No such file or directory") {
        EXIT_UNAVAILABLE
    } else if error.contains("could not acquire lock") || error.contains("storage is busy") {
        EXIT_TEMPFAIL
    } else {
        EXIT_USAGE
    }
}

fn identify_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::discovery_cache::normalize_reviewed_near_match_ids;
    use solstone_core_speaker_resolve::identify_cluster::{
        IdentifyClusterRequest, identify_cluster,
    };
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-identify-request-v1",
        &[
            "schema",
            "journal_root",
            "cluster_id",
            "name",
            "entity_id",
            "resolve_only",
            "create_new",
            "entity_type",
            "request_id",
            "reviewed_near_match_entity_ids",
            "caller",
            "actor",
            "encoder",
        ],
    )?;
    let object = &request_object;
    let reviewed_near_match_entity_ids =
        match normalize_reviewed_near_match_ids(object.get("reviewed_near_match_entity_ids")) {
            Ok(ids) => ids,
            Err(error) => return Ok(error.invalid_request_response()),
        };
    let request = IdentifyClusterRequest {
        journal_root: PathBuf::from(required_string(object, "journal_root")?),
        cluster_id: required_i64(object, "cluster_id")?,
        name: optional_string(object, "name")?,
        entity_id: optional_string(object, "entity_id")?,
        resolve_only: required_bool(object, "resolve_only")?,
        create_new: required_bool(object, "create_new")?,
        entity_type: required_string(object, "entity_type")?,
        request_id: required_string(object, "request_id")?,
        reviewed_near_match_entity_ids,
        caller: optional_string(object, "caller")?.unwrap_or_default(),
        actor: optional_string(object, "actor")?,
    };
    identify_cluster(&request, &encoder(object)?).map_err(|error| error.to_string())
}

fn undo_identify_request(value: Value) -> Result<Value, String> {
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-undo-identify-request-v1",
        &["schema", "journal_root", "operation_id", "encoder"],
    )?;
    let object = &request_object;
    solstone_core_speaker_resolve::identify_undo::undo_identify_operation(
        &PathBuf::from(required_string(object, "journal_root")?),
        &required_string(object, "operation_id")?,
        &encoder(object)?,
    )
    .map_err(|error| error.to_string())
}

fn bootstrap_request(value: Value, imports: bool) -> Result<Value, String> {
    use solstone_core_speaker_resolve::bootstrap::{
        BootstrapOutcome, BootstrapRequest, SeedFromImportsOutcome, bootstrap_voiceprints,
        seed_from_imports,
    };
    let schema = if imports {
        "solstone-speaker-resolve-seed-from-imports-request-v1"
    } else {
        "solstone-speaker-resolve-bootstrap-voiceprints-request-v1"
    };
    let request_object = request_object(
        value,
        schema,
        &["schema", "journal_root", "encoder", "added_at", "dry_run"],
    )?;
    let object = &request_object;
    let request = BootstrapRequest {
        journal_root: PathBuf::from(required_string(object, "journal_root")?),
        encoder: encoder(object)?,
        added_at: required_i64(object, "added_at")?,
        dry_run: required_bool(object, "dry_run")?,
    };
    if imports {
        let outcome = seed_from_imports(&request).map_err(|error| error.to_string())?;
        Ok(match outcome {
            SeedFromImportsOutcome::IdentityInvalid => {
                json!({"status":"speaker_owner_identity_invalid"})
            }
            SeedFromImportsOutcome::NoOwnerCentroid => json!({"status":"no_owner_centroid"}),
            SeedFromImportsOutcome::Completed(stats) => json!({
                "status":"completed", "segments_scanned":stats.segments_scanned,
                "segments_with_speakers":stats.segments_with_speakers,
                "speakers_found":stats.speakers_found, "embeddings_saved":stats.embeddings_saved,
                "embeddings_skipped_owner":stats.embeddings_skipped_owner,
                "embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,
                "speakers_unmatched":stats.speakers_unmatched, "errors":stats.errors,
            }),
        })
    } else {
        let outcome = bootstrap_voiceprints(&request).map_err(|error| error.to_string())?;
        Ok(match outcome {
            BootstrapOutcome::IdentityInvalid => {
                json!({"status":"speaker_owner_identity_invalid"})
            }
            BootstrapOutcome::NoOwnerCentroid => json!({"status":"no_owner_centroid"}),
            BootstrapOutcome::Completed(stats) => json!({
                "status":"completed", "segments_scanned":stats.segments_scanned,
                "single_speaker_segments":stats.single_speaker_segments,
                "speakers_found":stats.speakers_found, "entities_created":stats.entities_created,
                "embeddings_saved":stats.embeddings_saved,
                "embeddings_skipped_owner":stats.embeddings_skipped_owner,
                "embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,
                "speakers_unmatched":stats.speakers_unmatched, "errors":stats.errors,
            }),
        })
    }
}

fn merge_names_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::bootstrap::{MergeNamesOutcome, merge_names};
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-merge-names-request-v1",
        &["schema", "journal_root", "alias_name", "canonical_name"],
    )?;
    let object = &request_object;
    let outcome = merge_names(
        &PathBuf::from(required_string(object, "journal_root")?),
        &required_string(object, "alias_name")?,
        &required_string(object, "canonical_name")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(match outcome {
        MergeNamesOutcome::Ambiguous {
            field,
            ambiguity_id,
            candidates,
        } => json!({
            "status":"ambiguous", "field":field, "ambiguity_id":ambiguity_id,
            "candidates":candidates.into_iter().map(|candidate| json!({
                "id":candidate.id, "name":candidate.name, "tier":candidate.tier as u8,
                "score":candidate.score,
            })).collect::<Vec<_>>(),
        }),
        MergeNamesOutcome::AliasNotFound => json!({"status":"alias_not_found"}),
        MergeNamesOutcome::CanonicalNotFound => json!({"status":"canonical_not_found"}),
        MergeNamesOutcome::SameEntity { entity_id } => {
            json!({"status":"same_entity","entity_id":entity_id})
        }
        MergeNamesOutcome::PrincipalEntity { entity_id } => {
            json!({"status":"principal_entity","entity_id":entity_id})
        }
        MergeNamesOutcome::Ready {
            alias_entity_id,
            canonical_entity_id,
        } => json!({
            "status":"ready", "alias_entity_id":alias_entity_id,
            "canonical_entity_id":canonical_entity_id,
        }),
    })
}

fn backfill_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::backfill::{BackfillRunRequest, run_backfill};
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-backfill-request-v1",
        &[
            "schema",
            "journal_root",
            "operation_id",
            "reattribute",
            "now_ms",
        ],
    )?;
    let object = &request_object;
    let result = run_backfill(&BackfillRunRequest {
        journal_root: PathBuf::from(required_string(object, "journal_root")?),
        operation_id: required_string(object, "operation_id")?,
        reattribute: required_bool(object, "reattribute")?,
        now_ms: required_i64(object, "now_ms")?,
    })
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "operation_id":result.operation_id, "total_count":result.total_count,
        "processed_count":result.processed_count, "skipped_count":result.skipped_count,
        "error_count":result.error_count,
        "error_segments":result.error_segments.into_iter().map(|error| json!({
            "day":error.segment.day, "stream":error.segment.stream,
            "segment_key":error.segment.segment_key, "detail":error.detail,
        })).collect::<Vec<_>>(),
        "pending_count":result.pending_count,
        "done":result.done,
    }))
}

fn backfill_status_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::backfill_operations::{
        backfill_operation_status, backfill_operations_path, load_backfill_operations,
    };
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-backfill-status-request-v1",
        &["schema", "journal_root", "operation_id"],
    )?;
    let object = &request_object;
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let operation_id = required_string(object, "operation_id")?;
    let status = backfill_operation_status(
        &load_backfill_operations(&backfill_operations_path(&root))
            .map_err(|error| error.to_string())?,
        &operation_id,
    )
    .map_err(|error| error.to_string())?;
    Ok(status.map_or_else(
        || json!({"status":"not_found","operation_id":operation_id}),
        |status| {
            json!({
                "status":if status.done { "done" } else { "resumable" },
                "operation_id":operation_id, "total_count":status.total_count,
                "completed_count":status.completed_count, "pending_count":status.pending_count,
                "error_count":status.error_count,
                "error_segments":status.error_segments.into_iter().map(|error| json!({
                    "day":error.segment.day, "stream":error.segment.stream,
                    "segment_key":error.segment.segment_key, "detail":error.detail,
                })).collect::<Vec<_>>(),
                "resumable":status.resumable, "done":status.done,
            })
        },
    ))
}

fn request_object(
    value: Value,
    schema: &str,
    fields: &[&str],
) -> Result<Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    if object.keys().any(|key| !fields.contains(&key.as_str()))
        || object.get("schema").and_then(Value::as_str) != Some(schema)
    {
        return Err(format!("invalid request schema: {schema}"));
    }
    Ok(object.clone())
}

fn encoder(object: &Map<String, Value>) -> Result<solstone_core_entity::EncoderIdentity, String> {
    let encoder = object
        .get("encoder")
        .and_then(Value::as_object)
        .ok_or_else(|| "encoder is required".to_owned())?;
    Ok(solstone_core_entity::EncoderIdentity {
        id: required_string(encoder, "id")?,
        sha256: required_string(encoder, "sha256")?,
        width: encoder
            .get("width")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "encoder width is required".to_owned())?,
    })
}

fn optional_string(object: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        _ => Err(format!("{name} must be a non-empty string or null")),
    }
}

fn required_bool(object: &Map<String, Value>, name: &str) -> Result<bool, String> {
    object
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{name} is required"))
}

fn required_i64(object: &Map<String, Value>, name: &str) -> Result<i64, String> {
    object
        .get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{name} is required"))
}

fn parse_accumulation_request(
    value: Value,
) -> Result<solstone_core_speaker_resolve::voiceprint_accumulation::AccumulationRequest, String> {
    use solstone_core_entity::EncoderIdentity;
    use solstone_core_speaker_resolve::voiceprint_accumulation::{
        AccumulationEmbedding, AccumulationLabel, AccumulationRequest,
    };
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    const FIELDS: [&str; 8] = [
        "schema",
        "journal_root",
        "segment",
        "now_ms",
        "encoder",
        "labels",
        "embeddings",
        "entity_ids",
    ];
    if object.keys().any(|key| !FIELDS.contains(&key.as_str()))
        || object.get("schema").and_then(Value::as_str)
            != Some("solstone-speaker-resolve-accumulate-voiceprints-request-v1")
    {
        return Err("invalid accumulation request schema".to_owned());
    }
    let segment = object
        .get("segment")
        .and_then(Value::as_object)
        .ok_or_else(|| "segment is required".to_owned())?;
    let string = |object: &Map<String, Value>, key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is required"))
    };
    let encoder = object
        .get("encoder")
        .and_then(Value::as_object)
        .ok_or_else(|| "encoder is required".to_owned())?;
    let labels = serde_json::from_value::<Vec<AccumulationLabel>>(
        object
            .get("labels")
            .cloned()
            .ok_or_else(|| "labels is required".to_owned())?,
    )
    .map_err(|_| "invalid labels".to_owned())?;
    let embeddings = serde_json::from_value::<Vec<AccumulationEmbedding>>(
        object
            .get("embeddings")
            .cloned()
            .ok_or_else(|| "embeddings is required".to_owned())?,
    )
    .map_err(|_| "invalid embeddings".to_owned())?;
    let entity_ids = serde_json::from_value::<Vec<String>>(
        object
            .get("entity_ids")
            .cloned()
            .ok_or_else(|| "entity_ids is required".to_owned())?,
    )
    .map_err(|_| "invalid entity_ids".to_owned())?;
    Ok(AccumulationRequest {
        journal_root: PathBuf::from(string(object, "journal_root")?),
        day: string(segment, "day")?,
        stream: string(segment, "stream")?,
        segment_key: string(segment, "segment_key")?,
        source: string(segment, "source")?,
        now_ms: object
            .get("now_ms")
            .and_then(Value::as_i64)
            .ok_or_else(|| "now_ms is required".to_owned())?,
        encoder: EncoderIdentity {
            id: string(encoder, "id")?,
            sha256: string(encoder, "sha256")?,
            width: object
                .get("encoder")
                .and_then(|value| value.get("width"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| "encoder width is required".to_owned())?,
        },
        labels,
        embeddings,
        entity_ids,
    })
}

fn write_owner_centroid_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_centroid::{
        OwnerCentroidWriteError, OwnerCentroidWriteInput, write_owner_centroid,
    };
    let fields = owner_fields(value)?;
    match write_owner_centroid(
        &fields.root,
        &fields.principal,
        &OwnerCentroidWriteInput {
            centroid: fields.centroid,
            cluster_size: fields.cluster_size,
            timestamp: fields.timestamp,
            evidence_tier: fields.evidence_tier,
        },
    ) {
        Ok(()) => Ok(json!({"status":"written"})),
        Err(
            error @ (OwnerCentroidWriteError::IdentityInvalid
            | OwnerCentroidWriteError::TargetMismatch { .. }),
        ) => Ok(json!({"status":owner_centroid_refusal_reason(&error)})),
        Err(error) => Err(error.to_string()),
    }
}

fn rebuild_owner_centroid_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_centroid::{
        OwnerCentroidRebuildInput, OwnerCentroidWriteError, rebuild_owner_centroid,
    };
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    let fields = owner_fields(Value::Object(object.clone()))?;
    let evidence_hash = object
        .get("evidence_hash")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "evidence_hash is required".to_owned())?
        .to_owned();
    let p25 = object
        .get("evidence_intra_cosine_p25")
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .ok_or_else(|| "evidence_intra_cosine_p25 is required".to_owned())?;
    let outcome = match rebuild_owner_centroid(
        &fields.root,
        &fields.principal,
        &OwnerCentroidRebuildInput {
            centroid: fields.centroid,
            embeddings_count: fields.cluster_size,
            timestamp: fields.timestamp,
            evidence_hash,
            evidence_intra_cosine_p25: p25,
            evidence_tier: fields.evidence_tier,
            override_regression: object
                .get("override")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    ) {
        Ok(outcome) => outcome,
        Err(
            error @ (OwnerCentroidWriteError::IdentityInvalid
            | OwnerCentroidWriteError::TargetMismatch { .. }),
        ) => {
            return Ok(json!({"status":owner_centroid_refusal_reason(&error)}));
        }
        Err(error) => return Err(error.to_string()),
    };
    let outcome = serde_json::to_value(outcome).map_err(|error| error.to_string())?;
    Ok(json!({"outcome": outcome}))
}

fn owner_centroid_refusal_reason(
    error: &solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteError,
) -> &'static str {
    use solstone_core_speaker_resolve::owner_centroid::OwnerCentroidWriteError;
    use solstone_core_speaker_resolve::{
        OWNER_IDENTITY_INVALID_REASON, OWNER_TARGET_MISMATCH_REASON,
    };

    match error {
        OwnerCentroidWriteError::IdentityInvalid => OWNER_IDENTITY_INVALID_REASON,
        OwnerCentroidWriteError::TargetMismatch { .. } => OWNER_TARGET_MISMATCH_REASON,
        _ => unreachable!("owner centroid refusal reason is only called for admission errors"),
    }
}

fn write_owner_candidate_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_candidate::{OwnerCandidate, write_owner_candidate};
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let candidate = OwnerCandidate {
        centroid: f32_values(object.get("centroid"))?,
        cluster_size: required_i32(object, "cluster_size")?,
        threshold: required_f32(object, "threshold")?,
        version: required_string(object, "version")?,
        evidence_tier: required_string(object, "evidence_tier")?,
    };
    write_owner_candidate(&root, &candidate).map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn read_owner_candidate_request(value: Value) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    let candidate = solstone_core_speaker_resolve::owner_candidate::load_owner_candidate(
        &PathBuf::from(required_string(object, "journal_root")?),
    )
    .map_err(|error| error.to_string())?;
    let candidate = serde_json::to_value(candidate).map_err(|error| error.to_string())?;
    Ok(json!({"candidate": candidate}))
}

fn clear_owner_candidate_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-clear-owner-candidate-request-v1",
        &["schema", "journal_root"],
    )?;
    let removed = solstone_core_speaker_resolve::owner_candidate::clear_owner_candidate(
        &PathBuf::from(required_string(&object, "journal_root")?),
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "status":"cleared",
        "removed":matches!(removed, solstone_core_journal_io::Removed::Unlinked),
    }))
}

fn write_voiceprint_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-write-voiceprint-request-v1",
        &[
            "schema",
            "journal_root",
            "entity_id",
            "embedding",
            "metadata",
            "encoder",
        ],
    )?;
    let metadata = required_object_value(&object, "metadata")?;
    solstone_core_speaker_resolve::direct_voiceprints::write_voiceprint(
        &PathBuf::from(required_string(&object, "journal_root")?),
        &required_string(&object, "entity_id")?,
        f32_values_named(object.get("embedding"), "embedding")?,
        Value::Object(metadata),
        &encoder(&object)?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn screen_owner_contamination_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::owner_contamination_screen::{
        ContaminationProbe, screen_owner_contamination,
    };

    let object = request_object(
        value,
        "solstone-speaker-resolve-screen-owner-contamination-request-v1",
        &[
            "schema",
            "journal_root",
            "day",
            "stream",
            "segment_key",
            "source",
            "sentence_id",
            "encoder",
        ],
    )?;
    let root = PathBuf::from(required_string(&object, "journal_root")?);
    let probe = ContaminationProbe {
        day: required_string(&object, "day")?,
        stream: required_string(&object, "stream")?,
        segment_key: required_string(&object, "segment_key")?,
        source: required_string(&object, "source")?,
        sentence_id: required_i64(&object, "sentence_id")?,
    };
    let screen = screen_owner_contamination(&root, &probe, &encoder(&object)?)
        .map_err(|error| error.to_string())?;
    serde_json::to_value(screen).map_err(|error| error.to_string())
}

fn remove_voiceprint_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-remove-voiceprint-request-v1",
        &["schema", "journal_root", "entity_id", "key", "encoder"],
    )?;
    let key = required_object_value(&object, "key")?;
    let report = solstone_core_speaker_resolve::direct_voiceprints::remove_voiceprint(
        &PathBuf::from(required_string(&object, "journal_root")?),
        &required_string(&object, "entity_id")?,
        Value::Object(key.clone()),
        &encoder(&object)?,
    )
    .map_err(|error| error.to_string())?;
    let outcome = if report.removed_count == 0 {
        "not_found"
    } else if report.file_removed {
        "unlinked"
    } else {
        "rewritten"
    };
    Ok(json!({
        "outcome":outcome,
        "entity_id":required_string(&object, "entity_id")?,
        "keys_removed":if report.removed_count == 0 { Vec::<Value>::new() } else { vec![Value::Object(key)] },
        "file_deleted":report.file_removed,
    }))
}

fn backfill_voiceprint_last_seen_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-backfill-voiceprint-last-seen-request-v1",
        &[
            "schema",
            "journal_root",
            "entity_id",
            "last_seen_ts",
            "encoder",
        ],
    )?;
    let target_ts = required_i64(&object, "last_seen_ts")?;
    let updates = solstone_core_entity::rewrite_voiceprint_metadata(
        &PathBuf::from(required_string(&object, "journal_root")?),
        &required_string(&object, "entity_id")?,
        &encoder(&object)?,
        |rows| {
            let mut changed = 0;
            for row in rows {
                let Some(metadata) = row.as_object_mut() else {
                    continue;
                };
                let current = metadata.get("last_seen_ts").and_then(Value::as_i64);
                if current.is_none_or(|current| current < target_ts) {
                    metadata.insert("last_seen_ts".to_owned(), Value::from(target_ts));
                    changed += 1;
                }
            }
            changed
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"rows_written":updates}))
}

fn write_stub_labels_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-write-stub-labels-request-v1",
        &["schema", "journal_root", "segment", "reason"],
    )?;
    solstone_core_speaker_id::labels::write_stub_labels(
        &segment_dir(&object)?,
        &required_string(&object, "reason")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn write_full_labels_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-write-full-labels-request-v1",
        &["schema", "journal_root", "segment", "labels", "metadata"],
    )?;
    let labels = object
        .get("labels")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "labels is required".to_owned())?;
    solstone_core_speaker_id::labels::write_full_labels(
        &segment_dir(&object)?,
        labels,
        &required_object_value(&object, "metadata")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn patch_labels_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-patch-labels-request-v1",
        &[
            "schema",
            "journal_root",
            "segment",
            "patches",
            "allow_insert",
        ],
    )?;
    let patches = object
        .get("patches")
        .and_then(Value::as_object)
        .ok_or_else(|| "patches is required".to_owned())?
        .iter()
        .map(|(sentence_id, fields)| {
            Ok((
                sentence_id
                    .parse::<i64>()
                    .map_err(|_| "patch sentence ID must be an integer".to_owned())?,
                fields
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "patch fields must be an object".to_owned())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    solstone_core_speaker_id::labels::patch_labels(
        &segment_dir(&object)?,
        &patches,
        required_bool(&object, "allow_insert")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"written"}))
}

fn restore_label_rows_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-restore-label-rows-request-v1",
        &["schema", "journal_root", "segment", "restorations"],
    )?;
    let restorations = object
        .get("restorations")
        .and_then(Value::as_array)
        .ok_or_else(|| "restorations is required".to_owned())?
        .iter()
        .map(parse_label_restoration)
        .collect::<Result<Vec<_>, _>>()?;
    let report =
        solstone_core_speaker_id::labels::restore_label_rows(&segment_dir(&object)?, &restorations)
            .map_err(|error| error.to_string())?;
    Ok(json!({
        "restored_count":report.restored_count,
        "removed_inserted_count":report.removed_inserted_count,
        "patched_existing_count":report.patched_existing_count,
        "skipped_count":report.skipped_count,
        "skipped_reasons":{"missing":report.missing_count,"changed":report.changed_count},
    }))
}

fn append_correction_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-append-correction-request-v1",
        &["schema", "journal_root", "segment", "correction"],
    )?;
    solstone_core_speaker_id::corrections::append_correction(
        &segment_dir(&object)?,
        required_object_value(&object, "correction")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({"status":"appended"}))
}

fn wipe_speaker_artifacts_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-wipe-speaker-artifacts-request-v1",
        &["schema", "journal_root", "dry_run"],
    )?;
    let report = solstone_core_speaker_resolve::artifact_wipe::wipe_speaker_artifacts(
        &PathBuf::from(required_string(&object, "journal_root")?),
        required_bool(&object, "dry_run")?,
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_value(report).map_err(|error| error.to_string())
}

fn segment_dir(object: &Map<String, Value>) -> Result<PathBuf, String> {
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let segment = object
        .get("segment")
        .and_then(Value::as_object)
        .ok_or_else(|| "segment is required".to_owned())?;
    const FIELDS: [&str; 3] = ["day", "stream", "segment_key"];
    if segment.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err("invalid segment request".to_owned());
    }
    let day = required_string(segment, "day")?;
    let stream = required_string(segment, "stream")?;
    let segment_key = required_string(segment, "segment_key")?;
    solstone_core_journal_io::contained_path(
        &root,
        &format!("chronicle/{day}/{stream}/{segment_key}"),
    )
    .map_err(|error| error.to_string())
}

fn required_object_value(
    object: &Map<String, Value>,
    name: &str,
) -> Result<Map<String, Value>, String> {
    object
        .get(name)
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| format!("{name} is required"))
}

fn parse_label_restoration(
    value: &Value,
) -> Result<solstone_core_speaker_id::labels::LabelRestoration, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "label restoration must be an object".to_owned())?;
    const FIELDS: [&str; 4] = [
        "sentence_id",
        "expected_current_label",
        "prior_state",
        "prior_label",
    ];
    if object.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err("invalid label restoration".to_owned());
    }
    Ok(solstone_core_speaker_id::labels::LabelRestoration {
        sentence_id: required_i64(object, "sentence_id")?,
        expected_current_label: object
            .get("expected_current_label")
            .cloned()
            .ok_or_else(|| "expected_current_label is required".to_owned())?,
        prior_state: required_string(object, "prior_state")?,
        prior_label: object.get("prior_label").cloned(),
    })
}

struct OwnerFields {
    root: PathBuf,
    principal: String,
    centroid: Vec<f32>,
    cluster_size: i32,
    timestamp: String,
    evidence_tier: String,
}

fn owner_fields(value: Value) -> Result<OwnerFields, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "request must be an object".to_owned())?;
    Ok(OwnerFields {
        root: PathBuf::from(required_string(object, "journal_root")?),
        principal: required_string(object, "principal_entity_id")?,
        centroid: f32_values(object.get("centroid"))?,
        cluster_size: required_i32(object, "cluster_size")?,
        timestamp: required_string(object, "timestamp")?,
        evidence_tier: required_string(object, "evidence_tier")?,
    })
}

fn required_string(object: &Map<String, Value>, name: &str) -> Result<String, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} is required"))
}
fn required_i32(object: &Map<String, Value>, name: &str) -> Result<i32, String> {
    object
        .get(name)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("{name} is required"))
}
fn required_f32(object: &Map<String, Value>, name: &str) -> Result<f32, String> {
    object
        .get(name)
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{name} is required"))
}
fn f32_values(value: Option<&Value>) -> Result<Vec<f32>, String> {
    f32_values_named(value, "centroid")
}
fn f32_values_named(value: Option<&Value>, name: &str) -> Result<Vec<f32>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{name} is required"))?
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|value| value as f32)
                .filter(|value| value.is_finite())
                .ok_or_else(|| format!("invalid {name} float values"))
        })
        .collect()
}

fn run_speaker_transcript_write() -> ExitCode {
    let mut input = Vec::new();
    let read_result = io::stdin()
        .lock()
        .take((MAX_JSON_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut input);
    if let Err(error) = read_result {
        eprint_speaker_transcript_write_error("internal-error", &error.to_string());
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    if input.len() > MAX_JSON_STDIN_BYTES {
        eprint_speaker_transcript_write_error(
            "malformed-request",
            "stdin request exceeds the JSON input limit",
        );
        return ExitCode::from(EXIT_USAGE);
    }
    match solstone_core_speaker_id::writer::write_request(&input) {
        Ok(response) => {
            let output = json!({
                "schema": solstone_core_speaker_id::writer::RESPONSE_SCHEMA,
                "jsonl_path": response.jsonl_path,
                "npz_path": response.npz_path,
                "statement_count": response.statement_count,
                "embedding_row_count": response.embedding_row_count,
            });
            match serde_json::to_string(&output) {
                Ok(output) => {
                    println!("{output}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprint_speaker_transcript_write_error("internal-error", &error.to_string());
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        }
        Err(error) => {
            let exit = speaker_transcript_write_exit_code(&error);
            eprint_speaker_transcript_write_error(error.reason(), &error.detail());
            ExitCode::from(exit)
        }
    }
}

fn speaker_transcript_write_exit_code(
    error: &solstone_core_speaker_id::writer::SpeakerTranscriptWriteError,
) -> u8 {
    use solstone_core_speaker_id::writer::SpeakerTranscriptWriteError;

    match error {
        SpeakerTranscriptWriteError::MalformedRequest { .. }
        | SpeakerTranscriptWriteError::UnknownSchema { .. }
        | SpeakerTranscriptWriteError::MissingStatementId { .. }
        | SpeakerTranscriptWriteError::InvalidStatementId { .. }
        | SpeakerTranscriptWriteError::DuplicateStatementId { .. }
        | SpeakerTranscriptWriteError::InvalidStatement { .. }
        | SpeakerTranscriptWriteError::InvalidHeader { .. }
        | SpeakerTranscriptWriteError::InvalidOutputPath { .. }
        | SpeakerTranscriptWriteError::DestinationExists { .. } => EXIT_USAGE,
        SpeakerTranscriptWriteError::PayloadUnreadable { .. }
        | SpeakerTranscriptWriteError::PayloadInvalid { .. }
        | SpeakerTranscriptWriteError::PayloadNonFinite { .. } => EXIT_UNAVAILABLE,
        SpeakerTranscriptWriteError::OutputUnwritable { .. }
        | SpeakerTranscriptWriteError::NpzVerificationFailed { .. }
        | SpeakerTranscriptWriteError::Internal { .. } => EXIT_TEMPFAIL,
    }
}

fn eprint_speaker_transcript_write_error(reason: &str, detail: &str) {
    eprintln!(
        "{}",
        json!({
            "schema": "solstone-speaker-transcript-write-error-v1",
            "reason": reason,
            "detail": detail,
        })
    );
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
        LocalCommand::Install(command) => run_local_install(command),
        LocalCommand::Generate => run_local_generate_json(solstone_core_local::generate),
    }
}

fn run_assets() -> ExitCode {
    let mut stdout = io::stdout().lock();
    if stdout
        .write_all(solstone_core_assets::assets_json().as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn run_generate(command: GenerateCommand) -> ExitCode {
    match command {
        GenerateCommand::Contract => {
            print!("{}", solstone_core_generate::contract_source());
            ExitCode::SUCCESS
        }
        GenerateCommand::Malformed => generate_protocol_exit(
            None,
            "malformed-request",
            generate_selector_detail(),
            generate_exit_code("malformed_request"),
        ),
        GenerateCommand::OneShot => run_generate_one_shot(),
        GenerateCommand::Session(options) => run_generate_session(options),
    }
}

/// Run the native cogitate process boundary.
///
/// `cogitate_wire_contract.json` is the source of truth for which registered
/// Cortex kinds are native producers and which are consumer/transport-side.
/// Cortex writes a separate durable use log with consumer-added `use_id` and
/// `exit_code`, synthesized `info.message` and `error` records, and the
/// `_active.jsonl` to `.jsonl` rename (`cortex.py:707-733,1379-1384`). This
/// subcommand only streams its NDJSON boundary. It replaces the in-process
/// loop, policy, prompt assembly, and provider resolution formerly performed
/// by the retired Python tool-using runtime; Python still owns post-hooks, output files,
/// provenance, no-output gating, and reuse (`talents.py:1518-1619`).
fn run_cogitate(command: CogitateCommand) -> ExitCode {
    match command {
        CogitateCommand::Contract => {
            print!("{}", solstone_core_cogitate_wire::contract_source());
            ExitCode::SUCCESS
        }
        CogitateCommand::TalentContract => {
            let contract = talent_contract::talent_contract();
            println!(
                "{}",
                serde_json::to_string_pretty(&contract).expect("talent contract serializes")
            );
            ExitCode::SUCCESS
        }
        CogitateCommand::Malformed => {
            eprintln!("cogitate: expected --contract, --talent-contract, or --one-shot");
            ExitCode::from(EXIT_DATAERR)
        }
        CogitateCommand::OneShot => run_cogitate_one_shot(),
    }
}

fn run_cogitate_one_shot() -> ExitCode {
    // One-shot cogitate has one complete request, rather than generate's
    // multiplexed session framing, so a direct read-to-string is sufficient.
    let mut raw = String::new();
    if let Err(error) = io::stdin().lock().read_to_string(&mut raw) {
        eprintln!("cogitate stdin I/O error: {error}");
        return ExitCode::from(EXIT_INTERNAL_FAILURE);
    }
    let request = match solstone_core_cogitate_wire::CogitateRequest::parse(&raw) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(EXIT_DATAERR);
        }
    };
    // Dry runs deliberately avoid endpoint resolution, allowing callers to
    // validate a request without any provider configuration.
    if request.dry_run {
        return match solstone_core_cogitate_wire::serialize_dry_run(&request) {
            Ok(value) => {
                write_ndjson_line_or_exit(&value);
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(EXIT_INTERNAL_FAILURE)
            }
        };
    }
    let config = match read_journal_config(&request.journal_root) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("could not read journal config: {error}");
            return ExitCode::from(EXIT_INTERNAL_FAILURE);
        }
    };
    let (_, lane) = solstone_core_generate_wire::resolve_lane(&config);
    let mut sink = StdoutEventSink;
    let mut provider = match lane {
        lane @ (solstone_core_generate_wire::LaneOutcome::BundledLocal
        | solstone_core_generate_wire::LaneOutcome::ByoEndpoint(_)
        | solstone_core_generate_wire::LaneOutcome::ConfidentialEndpoint(_)
        | solstone_core_generate_wire::LaneOutcome::Anthropic
        | solstone_core_generate_wire::LaneOutcome::OpenAi
        | solstone_core_generate_wire::LaneOutcome::Google) => {
            solstone_core_cogitate_wire::DispatchConverseProvider::from_lane(
                &request,
                config,
                lane,
                solstone_core_cogitate_wire::EndpointOverrides::from_process(),
            )
            .expect("executable cogitate lane constructs a provider")
        }
        solstone_core_generate_wire::LaneOutcome::NoEngine => {
            emit_cogitate_preflight_error(
                &mut sink,
                &request,
                "no_engine_configured",
                "no provider is configured for this journal",
            );
            return ExitCode::SUCCESS;
        }
        solstone_core_generate_wire::LaneOutcome::UnimplementedLane => {
            emit_cogitate_preflight_error(
                &mut sink,
                &request,
                "unimplemented_lane",
                "the configured provider is not implemented for cogitate",
            );
            return ExitCode::SUCCESS;
        }
        // `resolve_lane` cannot return attestation failures: confidential
        // converse reports those through `ConverseFailure` inside its arm.
        _ => unreachable!("resolve_lane never returns a failure outcome"),
    };
    let mut slot = solstone_core_cogitate_tools::NoopSlotLease;
    let mut tools = solstone_core_cogitate_runtime::CogitateToolExecutor::new(
        &request.journal_root,
        request.read_call_budget,
        &mut slot,
    );
    match solstone_core_cogitate_wire::run_or_dry_run(
        &request,
        &mut provider,
        &mut tools,
        &mut sink,
    ) {
        Ok(solstone_core_cogitate_wire::NativeRun::Completed(_)) => ExitCode::SUCCESS,
        Ok(solstone_core_cogitate_wire::NativeRun::DryRun(value)) => {
            write_ndjson_line_or_exit(&value);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_INTERNAL_FAILURE)
        }
    }
}

fn emit_cogitate_preflight_error(
    sink: &mut StdoutEventSink,
    request: &solstone_core_cogitate_wire::CogitateRequest,
    reason_code: &str,
    error_text: &str,
) {
    solstone_core_cogitate_runtime::EventSink::emit(
        sink,
        solstone_core_cogitate_runtime::RuntimeEvent::Terminal {
            outcome: solstone_core_cogitate_runtime::RunOutcome {
                reason_code: Some(reason_code.to_owned()),
                error_text: Some(error_text.to_owned()),
                result: None,
                usage: solstone_core_cogitate_runtime::Usage::default(),
                raw_payload: None,
                terminal: true,
                correlation_id: request.correlation_id.clone(),
                provider_failure: None,
            },
        },
    );
}

struct StdoutEventSink;

impl solstone_core_cogitate_runtime::EventSink for StdoutEventSink {
    fn emit(&mut self, event: solstone_core_cogitate_runtime::RuntimeEvent) {
        match solstone_core_cogitate_wire::serialize_event_validated(event) {
            Ok(value) => write_ndjson_line_or_exit(&value),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(EXIT_INTERNAL_FAILURE.into());
            }
        }
    }
}

fn write_ndjson_line_or_exit(value: &Value) {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    if stdout
        .write_all(value.to_string().as_bytes())
        .and_then(|()| stdout.write_all(b"\n"))
        .and_then(|()| stdout.flush())
        .is_err()
    {
        // AC #16: EventSink cannot return Result, so terminate at the exact
        // failed write. Another final event cannot reach a dead pipe; process
        // exit belongs in this binary boundary, never in a subsystem crate.
        std::process::exit(EXIT_IOERR.into());
    }
}

fn generate_selector_detail() -> String {
    let session_selector = solstone_core_generate::contract()["framing"]["session"]["selector"]
        .as_str()
        .expect("session selector is a fixture string");
    format!("expected --contract, --one-shot, or {session_selector}")
}

fn run_generate_one_shot() -> ExitCode {
    let raw = match read_generate_stdin() {
        Ok(raw) => raw,
        Err(GenerateStdinError::InvalidUtf8(detail)) => {
            return generate_protocol_exit(
                None,
                "malformed-request",
                detail,
                generate_exit_code("malformed_request"),
            );
        }
        Err(GenerateStdinError::Io(detail)) => {
            return generate_protocol_exit(
                None,
                "internal-failure",
                detail,
                generate_exit_code("internal_failure"),
            );
        }
        Err(GenerateStdinError::TooLarge) => {
            return generate_protocol_exit(
                None,
                "internal-failure",
                "stdin exceeds 64 MiB".to_owned(),
                generate_exit_code("internal_failure"),
            );
        }
    };
    let request = match solstone_core_generate_wire::parse_one_shot_request(&raw) {
        Ok(request) => request,
        Err(detail) => {
            return generate_protocol_exit(
                None,
                "malformed-request",
                detail,
                generate_exit_code("malformed_request"),
            );
        }
    };
    let request_id = request.id.clone();
    let endpoint_runtime = solstone_core_generate_wire::EndpointRuntime::default();
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => {
            return generate_protocol_exit(
                request_id,
                "internal-failure",
                journal_path_error_detail(error),
                generate_exit_code("internal_failure"),
            );
        }
    };
    let response = match generate_response_for_request(&request, &endpoint_runtime, &journal) {
        Ok(response) => response,
        Err(detail) => {
            return generate_protocol_exit(
                request_id,
                "internal-failure",
                detail,
                generate_exit_code("internal_failure"),
            );
        }
    };
    match write_generate_response(&response) {
        Ok(()) => ExitCode::from(generate_exit_code("response")),
        Err(detail) => generate_protocol_exit(
            request_id,
            "internal-failure",
            detail,
            generate_exit_code("internal_failure"),
        ),
    }
}

fn generate_response_for_request(
    request: &solstone_core_generate::GenerateRequest,
    endpoint_runtime: &solstone_core_generate_wire::EndpointRuntime,
    journal: &Path,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    let request_id = request.id.clone();
    let config = match read_journal_config(journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => return Err(format!("could not read journal config: {error}")),
    };
    let (provider, outcome) = solstone_core_generate_wire::resolve_lane(&config);
    let response = match outcome {
        solstone_core_generate_wire::LaneOutcome::BundledLocal => {
            match solstone_core_generate_wire::bundled_generate(request, journal) {
                Ok(solstone_core_local::GenerateResult::Success(mut success)) => {
                    let usage = success
                        .usage
                        .as_ref()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|error| error.to_string())?
                        .unwrap_or_else(|| json!({}));
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                            raw_response_snippet: None,
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response = generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                Ok(solstone_core_local::GenerateResult::Failure(failure)) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::BundledFailure(Box::new(
                                failure,
                            )),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
                Err(error) => {
                    return Err(format!("bundled local generate is unavailable: {error:?}"));
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::ByoEndpoint(endpoint) => {
            endpoint_result_response(
                solstone_core_generate_wire::endpoint_generate(
                    request,
                    journal,
                    &endpoint,
                    &config,
                    endpoint_runtime,
                ),
                request,
                journal,
                &provider,
                request_id.clone(),
            )?
        }
        solstone_core_generate_wire::LaneOutcome::ConfidentialEndpoint(endpoint) => {
            match solstone_core_generate_wire::confidential_generate(
                request,
                journal,
                &endpoint,
                &config,
                endpoint_runtime,
            ) {
                solstone_core_generate_wire::ConfidentialResult::Generated(success) => {
                    endpoint_result_response(
                        solstone_core_generate_wire::EndpointResult::Generated(success),
                        request,
                        journal,
                        &provider,
                        request_id.clone(),
                    )?
                }
                solstone_core_generate_wire::ConfidentialResult::Failed(failure) => {
                    endpoint_result_response(
                        solstone_core_generate_wire::EndpointResult::Failed(failure),
                        request,
                        journal,
                        &provider,
                        request_id.clone(),
                    )?
                }
                solstone_core_generate_wire::ConfidentialResult::AttestationNotVerified => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AttestationNotVerified,
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
                solstone_core_generate_wire::ConfidentialResult::AttestationFailed => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AttestationFailed,
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
                solstone_core_generate_wire::ConfidentialResult::AttestationStale => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AttestationStale,
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::Anthropic => {
            match solstone_core_generate_wire::anthropic_generate(request, &config) {
                solstone_core_generate_wire::AnthropicResult::Generated(mut success) => {
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &success.usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                            raw_response_snippet: success.raw_response_snippet.as_deref(),
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            anthropic_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                solstone_core_generate_wire::AnthropicResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::AnthropicFailure(failure),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::OpenAi => {
            match solstone_core_generate_wire::openai_generate(request, &config) {
                solstone_core_generate_wire::OpenAiResult::Generated(mut success) => {
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &success.usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                            raw_response_snippet: success.raw_response_snippet.as_deref(),
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            openai_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                solstone_core_generate_wire::OpenAiResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::OpenAiFailure(failure),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::Google => {
            match solstone_core_generate_wire::google_generate(request, &config) {
                solstone_core_generate_wire::GoogleResult::Generated(mut success) => {
                    let assessment = solstone_core_generate_wire::assess_provider_result(
                        solstone_core_generate_wire::ProviderResultView {
                            journal_path: journal,
                            context: &request.context,
                            model: &success.model,
                            text: &success.text,
                            finish_reason: &success.finish_reason,
                            usage: &success.usage,
                            json_output: request.json_output,
                            enforce_responsiveness: request.enforce_responsiveness,
                            raw_response_snippet: success.raw_response_snippet.as_deref(),
                        },
                    );
                    if let Some(error) = assessment.token_log_error {
                        eprintln!("generate token usage log failed: {error}");
                    }
                    if let Some(failure) = assessment.failure {
                        solstone_core_generate::GenerateResponse::Refused(
                            solstone_core_generate_wire::refusal_for(
                                &solstone_core_generate_wire::LaneOutcome::ValidationFailure(
                                    failure,
                                ),
                                &provider,
                                request_id.clone(),
                            ),
                        )
                    } else {
                        let schema_validation = apply_schema_validation(
                            &mut success.text,
                            request.json_schema.as_ref(),
                        );
                        let response =
                            google_generated_response(request, success, schema_validation)?;
                        solstone_core_generate::GenerateResponse::Generated(Box::new(response))
                    }
                }
                solstone_core_generate_wire::GoogleResult::Failed(failure) => {
                    solstone_core_generate::GenerateResponse::Refused(
                        solstone_core_generate_wire::refusal_for(
                            &solstone_core_generate_wire::LaneOutcome::GoogleFailure(failure),
                            &provider,
                            request_id.clone(),
                        ),
                    )
                }
            }
        }
        solstone_core_generate_wire::LaneOutcome::NoEngine
        | solstone_core_generate_wire::LaneOutcome::AttestationNotVerified
        | solstone_core_generate_wire::LaneOutcome::AttestationFailed
        | solstone_core_generate_wire::LaneOutcome::AttestationStale
        | solstone_core_generate_wire::LaneOutcome::UnimplementedLane => {
            solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate_wire::refusal_for(&outcome, &provider, request_id.clone()),
            )
        }
        solstone_core_generate_wire::LaneOutcome::BundledFailure(_)
        | solstone_core_generate_wire::LaneOutcome::EndpointFailure(_)
        | solstone_core_generate_wire::LaneOutcome::AnthropicFailure(_)
        | solstone_core_generate_wire::LaneOutcome::OpenAiFailure(_)
        | solstone_core_generate_wire::LaneOutcome::GoogleFailure(_)
        | solstone_core_generate_wire::LaneOutcome::ValidationFailure(_) => {
            unreachable!("lane resolution cannot return an arm failure")
        }
    };
    Ok(response)
}

fn endpoint_result_response(
    result: solstone_core_generate_wire::EndpointResult,
    request: &solstone_core_generate::GenerateRequest,
    journal: &std::path::Path,
    provider: &str,
    request_id: Option<String>,
) -> Result<solstone_core_generate::GenerateResponse, String> {
    match result {
        solstone_core_generate_wire::EndpointResult::Generated(mut success) => {
            let usage = success
                .usage
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or_else(|| json!({}));
            let assessment = solstone_core_generate_wire::assess_provider_result(
                solstone_core_generate_wire::ProviderResultView {
                    journal_path: journal,
                    context: &request.context,
                    model: &success.model,
                    text: &success.text,
                    finish_reason: &success.finish_reason,
                    usage: &usage,
                    json_output: request.json_output,
                    enforce_responsiveness: request.enforce_responsiveness,
                    raw_response_snippet: None,
                },
            );
            if let Some(error) = assessment.token_log_error {
                eprintln!("generate token usage log failed: {error}");
            }
            if let Some(failure) = assessment.failure {
                Ok(solstone_core_generate::GenerateResponse::Refused(
                    solstone_core_generate_wire::refusal_for(
                        &solstone_core_generate_wire::LaneOutcome::ValidationFailure(failure),
                        provider,
                        request_id,
                    ),
                ))
            } else {
                let schema_validation =
                    apply_schema_validation(&mut success.text, request.json_schema.as_ref());
                let response = endpoint_generated_response(request, success, schema_validation)?;
                Ok(solstone_core_generate::GenerateResponse::Generated(
                    Box::new(response),
                ))
            }
        }
        solstone_core_generate_wire::EndpointResult::Failed(failure) => {
            Ok(solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate_wire::refusal_for(
                    &solstone_core_generate_wire::LaneOutcome::EndpointFailure(failure),
                    provider,
                    request_id,
                ),
            ))
        }
    }
}

fn run_generate_session(options: GenerateSessionOptions) -> ExitCode {
    let config = match generate_session_config(&options) {
        Ok(config) => config,
        Err(detail) => {
            return generate_protocol_exit(
                None,
                "malformed-request",
                detail,
                generate_exit_code("malformed_request"),
            );
        }
    };
    let GenerateSessionConfig {
        session,
        explicit_journal,
    } = config;
    // The framing itself lives in the wire crate. What stays here is what only
    // the binary can decide: which lane answers a request, and what the process
    // does when the protocol cannot continue.
    let outcome = solstone_core_generate_wire::run_session(
        io::BufReader::new(io::stdin()),
        Box::new(io::stdout()),
        session,
        solstone_core_generate_wire::SessionHost {
            respond:
                move |request: &solstone_core_generate::GenerateRequest,
                      runtime: &solstone_core_generate_wire::EndpointRuntime| {
                    let journal = match &explicit_journal {
                        Some(path) => path.clone(),
                        None => resolve_process_journal_path()
                            .map(|resolved| resolved.path)
                            .map_err(journal_path_error_detail)?,
                    };
                    generate_response_for_request(request, runtime, &journal)
                },
            fail: |id, reason, detail| generate_protocol_exit_and_terminate(id, reason, detail),
            // Bare EOF means the caller disappeared: answer nothing further, write no
            // further usage, exit. The contract declares 0, 64 and 70 only, and the
            // reference implementation returns from its session loop here, so this exits 0.
            // Whether the abort deserves a distinct declared code is a contract question,
            // not one a port settles: 1 would collide with crash, kill and OOM, which is
            // the same collision the absent 69 exists to avoid.
            abort: || std::process::exit(0),
        },
    );
    match outcome {
        solstone_core_generate_wire::SessionOutcome::Completed => {
            ExitCode::from(generate_exit_code("response"))
        }
        solstone_core_generate_wire::SessionOutcome::ReaderDisconnected => generate_protocol_exit(
            None,
            "internal-failure",
            "session stdin reader disconnected".to_owned(),
            generate_exit_code("internal_failure"),
        ),
    }
}

struct GenerateSessionConfig {
    session: solstone_core_generate_wire::SessionConfig,
    explicit_journal: Option<PathBuf>,
}

fn generate_session_config(
    options: &GenerateSessionOptions,
) -> Result<GenerateSessionConfig, String> {
    let session = &solstone_core_generate::contract()["framing"]["session"];
    let selector = session["selector"]
        .as_str()
        .ok_or_else(|| "session selector is not a string".to_owned())?;
    let concurrency = &session["concurrency"];
    let flag = concurrency["flag"]
        .as_str()
        .ok_or_else(|| "session concurrency flag is not a string".to_owned())?;
    let journal_flag = session["journal"]["flag"]
        .as_str()
        .ok_or_else(|| "session journal flag is not a string".to_owned())?;
    let minimum = concurrency["minimum"]
        .as_u64()
        .ok_or_else(|| "session concurrency minimum is not an integer".to_owned())?;
    let line_limit_bytes = session["line_limit_bytes"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "session line limit is not a supported integer".to_owned())?;
    let terminal_schema = session["terminal"]["schema"]
        .as_str()
        .ok_or_else(|| "session terminal schema is not a string".to_owned())?
        .to_owned();
    let expected =
        || format!("expected {selector} {flag} <positive integer> [{journal_flag} <journal path>]");
    let mut max_in_flight = None;
    let mut explicit_journal = None;
    let mut pairs = options.arguments.chunks_exact(2);
    for pair in &mut pairs {
        let [actual_flag, value] = pair else {
            unreachable!("chunks_exact(2) yields pairs")
        };
        if actual_flag == OsStr::new(flag) {
            if max_in_flight.is_some() {
                return Err(format!("{flag} was provided more than once for {selector}"));
            }
            let value = value.to_str().ok_or_else(&expected)?;
            max_in_flight = Some(value.parse::<usize>().map_err(|_| expected())?);
        } else if actual_flag == OsStr::new(journal_flag) {
            if explicit_journal.is_some() {
                return Err(format!(
                    "{journal_flag} was provided more than once for {selector}"
                ));
            }
            explicit_journal = Some(PathBuf::from(value));
        } else {
            return Err(expected());
        }
    }
    if !pairs.remainder().is_empty() {
        return Err(expected());
    }
    let max_in_flight = max_in_flight.ok_or_else(expected)?;
    let minimum = usize::try_from(minimum)
        .map_err(|_| "session concurrency minimum is not a supported integer".to_owned())?;
    if max_in_flight < minimum {
        return Err(format!(
            "{flag} is below the fixture minimum of {minimum} for {selector}"
        ));
    }
    Ok(GenerateSessionConfig {
        session: solstone_core_generate_wire::SessionConfig {
            max_in_flight,
            line_limit_bytes,
            terminal_schema,
        },
        explicit_journal,
    })
}

fn generate_protocol_exit_and_terminate(id: Option<String>, reason: &str, detail: String) -> ! {
    let exit = generate_exit_code(if reason == "malformed-request" {
        "malformed_request"
    } else {
        "internal_failure"
    });
    let _ = generate_protocol_exit(id, reason, detail, exit);
    std::process::exit(i32::from(exit));
}

fn generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_local::GenerateSuccess,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let usage = success
        .usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| json!({}));
    let input_budget = success
        .input_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let request_budget = serde_json::to_value(&success.request_budget)
        .map(Some)
        .map_err(|error| error.to_string())?;
    let inference = serde_json::to_value(&success.inference)
        .map(Some)
        .map_err(|error| error.to_string())?;
    let hints_applied = applied_hints(request, true, true);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage,
        finish_reason: success.finish_reason,
        thinking: None,
        schema_validation,
        input_budget,
        request_budget,
        inference,
        hints_applied,
    })
}

/// Which request hints the boundary actually applied.
///
/// 🔴 A hint is reported only by a lane that HONOURED it, and the lanes differ:
/// measured, `bundled` reads `exclusive_admission` and the attempt index,
/// `endpoint` reads `exclusive_admission` to size its admission slot, and the
/// four cloud lanes read neither — their only mention of it is a test fixture.
///
/// ⚠ Two wrong rules were tried before this one, and both passed a real test
/// suite. The original reported `exclusive_admission` from every lane, so the
/// cloud lanes claimed an effect they never had. The correction after that keyed
/// off the response carrying an inference block, which is how the Python
/// reference decided it — but that gates on the *shape of the result*, not on
/// what was honoured, and it silenced the endpoint lane, which genuinely does
/// acquire the slot. ⛔ Do not re-derive this from the reference; it
/// under-reported.
fn applied_hints(
    request: &solstone_core_generate::GenerateRequest,
    honours_attempt_index: bool,
    honours_exclusive_admission: bool,
) -> Vec<String> {
    let mut hints = Vec::new();
    if honours_attempt_index && request.attempt_index != 0 {
        hints.push("attempt_index".to_owned());
    }
    if honours_exclusive_admission && request.exclusive_admission {
        hints.push("exclusive_admission".to_owned());
    }
    hints
}

fn endpoint_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::EndpointGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let usage = success
        .usage
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| json!({}));
    let input_budget = success
        .input_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let request_budget = success
        .request_budget
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let hints_applied = applied_hints(request, false, true);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage,
        finish_reason: success.finish_reason,
        thinking: None,
        schema_validation,
        input_budget,
        request_budget,
        inference: None,
        hints_applied,
    })
}

fn anthropic_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::AnthropicGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn openai_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::OpenAiGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn google_generated_response(
    request: &solstone_core_generate::GenerateRequest,
    success: solstone_core_generate_wire::GoogleGenerated,
    schema_validation: Option<Value>,
) -> Result<solstone_core_generate::GeneratedResponse, String> {
    let hints_applied = applied_hints(request, false, false);
    Ok(solstone_core_generate::GeneratedResponse {
        id: request.id.clone(),
        text: success.text,
        model: success.model,
        usage: success.usage,
        finish_reason: success.finish_reason,
        thinking: success.thinking,
        schema_validation,
        input_budget: None,
        request_budget: None,
        inference: None,
        hints_applied,
    })
}

fn apply_schema_validation(text: &mut String, schema: Option<&Value>) -> Option<Value> {
    schema.map(|schema| {
        let result = solstone_core_generate_wire::validate_schema_with_annotations(text, schema);
        *text = result.text;
        log_schema_validation(&result.validation);
        result.validation
    })
}

/// Schema validation is advisory, so a failure never changes the outcome and is
/// visible only in the response record — which reaches the caller and nothing
/// else.
///
/// ⚠ The reference emitted one operator-visible line per error and the port
/// dropped it. Nothing failed: the record still carried the block, so the
/// contract-level assertions passed and only the diagnostic disappeared. ⛔ On
/// stderr, never stdout — a line on stdout would corrupt the response stream,
/// which is what the test guarding this is named for.
fn log_schema_validation(validation: &Value) {
    let Some(errors) = validation.get("errors").and_then(Value::as_array) else {
        return;
    };
    for error in errors {
        let path = error.get("path").and_then(Value::as_str).unwrap_or("");
        let constraint = error
            .get("constraint")
            .and_then(Value::as_str)
            .unwrap_or("");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        eprintln!("schema_validation: {path}: {constraint}: {message}");
    }
}

enum GenerateStdinError {
    Io(String),
    TooLarge,
    InvalidUtf8(String),
}

fn read_generate_stdin() -> Result<String, GenerateStdinError> {
    let mut stdin = io::stdin().lock();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin
            .read(&mut chunk)
            .map_err(|error| GenerateStdinError::Io(format!("stdin I/O error: {error}")))?;
        if read == 0 {
            break;
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > MAX_LOCAL_GENERATE_STDIN_BYTES)
        {
            return Err(GenerateStdinError::TooLarge);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes)
        .map_err(|error| GenerateStdinError::InvalidUtf8(format!("stdin is not UTF-8: {error}")))
}

fn generate_exit_code(name: &str) -> u8 {
    solstone_core_generate::contract()["exit_codes"][name]
        .as_u64()
        .expect("generate contract exit code is an unsigned integer") as u8
}

fn generate_protocol_exit(id: Option<String>, reason: &str, detail: String, exit: u8) -> ExitCode {
    let error = solstone_core_generate::ProtocolError {
        id,
        reason: reason.to_owned(),
        detail,
    };
    match solstone_core_generate::encode_protocol_error(&error) {
        Ok(encoded) => {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(encoded.as_bytes());
            let _ = stderr.write_all(b"\n");
            let _ = stderr.flush();
        }
        Err(error) => eprintln!("generate protocol error encoding failed: {error}"),
    }
    ExitCode::from(exit)
}

fn write_generate_response(
    response: &solstone_core_generate::GenerateResponse,
) -> Result<(), String> {
    let encoded = solstone_core_generate::encode_one_shot_response(response)?;
    write_generate_response_to(&mut io::stdout().lock(), &encoded)
}

fn write_generate_response_to(writer: &mut impl Write, encoded: &str) -> Result<(), String> {
    writer
        .write_all(encoded.as_bytes())
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .map_err(|error| format!("stdout I/O error: {error}"))
}

fn journal_path_error_detail(error: JournalPathError) -> String {
    match error {
        JournalPathError::Config(error) => format!("could not resolve journal path: {error:?}"),
        JournalPathError::Home(error) => format!("could not resolve journal path: {error:?}"),
        JournalPathError::Create(error) => format!("could not resolve journal path: {error}"),
    }
}

fn run_local_install(command: InstallCommand) -> ExitCode {
    let input = match read_local_stdin(MAX_LOCAL_STDIN_BYTES) {
        Ok(input) => input,
        Err(LocalStdinError::Content) => return ExitCode::from(EXIT_USAGE),
        Err(LocalStdinError::Io) => return ExitCode::from(EXIT_IOERR),
    };
    let verb = match command {
        InstallCommand::PinsLocal => solstone_core_local::InstallVerb::PinsLocal,
        InstallCommand::PathsLocal => solstone_core_local::InstallVerb::PathsLocal,
        InstallCommand::FingerprintLocal => solstone_core_local::InstallVerb::FingerprintLocal,
        InstallCommand::VerifySha256 => solstone_core_local::InstallVerb::VerifySha256,
        InstallCommand::CudaTrust => solstone_core_local::InstallVerb::CudaTrust,
        InstallCommand::ManifestVulkan => solstone_core_local::InstallVerb::ManifestVulkan,
        InstallCommand::ManifestCuda => solstone_core_local::InstallVerb::ManifestCuda,
        InstallCommand::ManifestModel => solstone_core_local::InstallVerb::ManifestModel,
        InstallCommand::InspectLocal => solstone_core_local::InstallVerb::InspectLocal,
        InstallCommand::InspectParakeet => solstone_core_local::InstallVerb::InspectParakeet,
        InstallCommand::ProbeBinary => solstone_core_local::InstallVerb::ProbeBinary,
        InstallCommand::RunLocal => solstone_core_local::InstallVerb::RunLocal,
        InstallCommand::PinsParakeet => solstone_core_local::InstallVerb::PinsParakeet,
        InstallCommand::PathsParakeet => solstone_core_local::InstallVerb::PathsParakeet,
        InstallCommand::FingerprintParakeet => {
            solstone_core_local::InstallVerb::FingerprintParakeet
        }
        InstallCommand::RunParakeet => solstone_core_local::InstallVerb::RunParakeet,
    };
    match solstone_core_local::dispatch_install(verb, input) {
        Ok(envelope) => write_install_envelope(&envelope, ExitCode::SUCCESS),
        Err(error) => write_install_envelope(&error.envelope, ExitCode::from(error.exit_code)),
    }
}

fn write_install_envelope(
    envelope: &solstone_core_local::InstallEnvelope,
    exit: ExitCode,
) -> ExitCode {
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, envelope).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(EXIT_IOERR);
    }
    exit
}

fn run_local_json<T, O>(operation: impl FnOnce(T) -> O) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    run_local_json_with_limit(operation, MAX_LOCAL_STDIN_BYTES, "1 MiB")
}

fn run_local_generate_json<T, O>(operation: impl FnOnce(T) -> O) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    run_local_json_with_limit(operation, MAX_LOCAL_GENERATE_STDIN_BYTES, "64 MiB")
}

fn run_local_json_with_limit<T, O>(
    operation: impl FnOnce(T) -> O,
    max_bytes: usize,
    limit_label: &str,
) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    let input = match read_local_stdin(max_bytes) {
        Ok(input) => input,
        Err(LocalStdinError::Content) => {
            eprintln!("local command failed: stdin was not valid JSON within {limit_label}");
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

fn read_local_stdin<T: DeserializeOwned>(max_bytes: usize) -> Result<T, LocalStdinError> {
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
            .is_none_or(|next| next > max_bytes)
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
        BrainCommand::RefreshSession(options) => run_brain_refresh_session(options),
        BrainCommand::PrerequisiteRenewalSession(options) => {
            run_brain_prerequisite_renewal_session(options)
        }
        BrainCommand::Inspect(options) => run_brain_inspect(options),
        BrainCommand::Fingerprint => run_brain_fingerprint(),
    }
}

fn run_setup(args: solstone_core_setup::args::SetupArgs) -> ExitCode {
    solstone_core_setup::run_owner_setup_native(args)
}

fn run_doctor(args: solstone_core_doctor::args::DoctorArgs) -> ExitCode {
    let context = match solstone_core_doctor::context::CheckContext::production(args.port) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("doctor failed to resolve context: {error}");
            return ExitCode::from(1);
        }
    };
    let started = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let started_at = std::time::Instant::now();
    let results = solstone_core_doctor::run(&args, &context);
    if args.json {
        solstone_core_doctor::output::emit_json(&results);
    } else if args.jsonl {
        solstone_core_doctor::output::emit_jsonl(
            &results,
            &started,
            started_at.elapsed().as_millis(),
            args.port,
        );
    } else {
        solstone_core_doctor::output::emit_text(&results, args.verbose);
    }
    if solstone_core_doctor::vocabulary::results_failed(&results) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_brain_inspect(options: BrainInspectOptions) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let config = match read_journal_config(&line.path) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("brain inspect failed: could not read journal config: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let inspection =
        solstone_core_brain::inspect_brain_state(&line.path, &config, chrono::Utc::now());
    let active_fingerprint = active_brain_fingerprint(
        &line.path,
        &config,
        options.bundled_runtime_fingerprint_sha256,
    );
    // `inspection` is the transport form of Python's `inspect_brain_state`;
    // `active_fingerprint` serves both of its active-fingerprint read callers.
    let output = json!({
        "status": inspection_status_name(&inspection.status),
        "path": solstone_core_brain::brain_state_path(&line.path),
        "record": inspection.record,
        "projection": {
            "aggregate_state": inspection.projection.aggregate_state,
            "reason_code": inspection.projection.reason_code,
            "active_lane": inspection.projection.active_lane,
            "active_provider": inspection.projection.active_provider,
            "active_model": inspection.projection.active_model,
            "fingerprint_sha256": inspection.projection.fingerprint_sha256,
            "runtime_transition_in_progress": inspection.projection.runtime_transition_in_progress,
        },
        "reason_code": inspection.projection.reason_code,
        "error": inspection.error,
        "active_fingerprint": active_fingerprint,
    });
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        eprintln!("brain inspect failed: stdout I/O error");
        ExitCode::from(EXIT_IOERR)
    } else {
        ExitCode::SUCCESS
    }
}

fn inspection_status_name(status: &solstone_core_brain::InspectionStatus) -> &'static str {
    match status {
        solstone_core_brain::InspectionStatus::Ok => "ok",
        solstone_core_brain::InspectionStatus::Corrupt => "corrupt",
        solstone_core_brain::InspectionStatus::Unavailable => "unavailable",
    }
}

fn active_brain_fingerprint(
    journal_path: &std::path::Path,
    config: &Map<String, Value>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Value {
    let key = solstone_core_brain::load_existing_fingerprint_key(journal_path);
    brain_fingerprint_result(
        config,
        key.as_ref().map(|key| key.as_slice()),
        bundled_runtime_fingerprint_sha256,
    )
}

fn brain_fingerprint_result(
    config: &Map<String, Value>,
    hmac_key: Option<&[u8]>,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> Value {
    let resolution = solstone_core_brain::derive_active_brain_lane(config);
    let mut diagnostic = Map::new();
    let bundled_runtime = (resolution.lane.as_deref() == Some("bundled"))
        .then_some(bundled_runtime_fingerprint_sha256)
        .flatten();
    let (ok, fingerprint_sha256, reason_code) = match hmac_key {
        None => (false, None, Some("fingerprint_key_unavailable".to_owned())),
        Some(key) => match solstone_core_brain::build_active_brain_fingerprint(
            config,
            key,
            bundled_runtime.clone().map(Value::String),
        ) {
            Ok(Some(fingerprint)) => (true, Some(fingerprint), None),
            Ok(None) => (false, None, Some("fingerprint_not_available".to_owned())),
            Err(error) => {
                if error.0 == "configuration_invalid" {
                    diagnostic.insert(
                        "field".to_owned(),
                        Value::String("providers.active.provider".to_owned()),
                    );
                }
                (false, None, Some(error.0))
            }
        },
    };
    json!({
        "ok": ok,
        "fingerprint_sha256": fingerprint_sha256,
        "active_lane": resolution.lane,
        "active_provider": resolution.provider,
        "active_model": resolution.model,
        "reason_code": reason_code,
        "diagnostic": diagnostic,
        "bundled_runtime_fingerprint_sha256": bundled_runtime,
    })
}

struct BrainFingerprintRequest {
    config: Map<String, Value>,
    hmac_key: [u8; 32],
    bundled_runtime_fingerprint_sha256: Option<String>,
}

fn run_brain_fingerprint() -> ExitCode {
    let request = match read_bounded_json_stdin().and_then(parse_brain_fingerprint_request) {
        Ok(request) => request,
        Err(JsonStdinError::Content) => {
            eprintln!(
                "brain fingerprint failed: stdin was not a valid request JSON object within 1 MiB"
            );
            return ExitCode::from(EXIT_USAGE);
        }
        Err(JsonStdinError::Io) => {
            eprintln!("brain fingerprint failed: stdin I/O error");
            return ExitCode::from(EXIT_IOERR);
        }
    };
    let output = brain_fingerprint_result(
        &request.config,
        Some(&request.hmac_key),
        request.bundled_runtime_fingerprint_sha256,
    );
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err()
        || stdout.write_all(b"\n").is_err()
        || stdout.flush().is_err()
    {
        eprintln!("brain fingerprint failed: stdout I/O error");
        ExitCode::from(EXIT_IOERR)
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_brain_fingerprint_request(
    request: Map<String, Value>,
) -> Result<BrainFingerprintRequest, JsonStdinError> {
    const FIELDS: [&str; 3] = [
        "config",
        "hmac_key_hex",
        "bundled_runtime_fingerprint_sha256",
    ];
    if request.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err(JsonStdinError::Content);
    }
    let config = request
        .get("config")
        .and_then(Value::as_object)
        .cloned()
        .ok_or(JsonStdinError::Content)?;
    let hmac_key = request
        .get("hmac_key_hex")
        .and_then(Value::as_str)
        .and_then(decode_hmac_key)
        .ok_or(JsonStdinError::Content)?;
    let bundled_runtime_fingerprint_sha256 = match request.get("bundled_runtime_fingerprint_sha256")
    {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(JsonStdinError::Content),
    };
    Ok(BrainFingerprintRequest {
        config,
        hmac_key,
        bundled_runtime_fingerprint_sha256,
    })
}

fn decode_hmac_key(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn run_brain_refresh_session(options: BrainRefreshSessionOptions) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let (expected, expect_absent) = match options.expect {
        Some(BrainRefreshExpectArg::Absent) => (None, true),
        Some(BrainRefreshExpectArg::Sha256(fingerprint)) => (Some(fingerprint), false),
        None => (None, false),
    };
    let before_revision = brain_record_revision(&line.path);
    let bundled_runtime_fingerprint_sha256 = options.bundled_runtime_fingerprint_sha256;
    let begin = solstone_core_brain::begin_refresh(
        &line.path,
        chrono::Utc::now(),
        options.run_id,
        expected.as_deref(),
        expect_absent,
        bundled_runtime_fingerprint_sha256.clone(),
    );
    let permit = match begin {
        Ok(Some(permit)) => permit,
        Ok(None)
            if brain_record_was_written(before_revision, brain_record_revision(&line.path)) =>
        {
            return write_refresh_projection(&line.path);
        }
        Ok(None) => {
            return write_session_result(json!({
                "schema": REFRESH_RESULT_SCHEMA,
                "kind": "not_started",
                "status": "no_permit",
                "reason": "lease_held",
            }));
        }
        Err(solstone_core_brain::BeginRefreshError::ExpectedFingerprintStale(error)) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_DATAERR);
        }
        Err(solstone_core_brain::BeginRefreshError::InvalidArgument(error)) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
        Err(solstone_core_brain::BeginRefreshError::Writer(error)) => {
            eprintln!("brain refresh failed: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    run_refresh_session_loop(
        line.path,
        permit,
        bundled_runtime_fingerprint_sha256,
        session_input_timeout(),
    )
}

fn run_brain_prerequisite_renewal_session(
    options: BrainPrerequisiteRenewalSessionOptions,
) -> ExitCode {
    let line = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let bundled_runtime_fingerprint_sha256 = options.bundled_runtime_fingerprint_sha256;
    let expected_fingerprint_sha256 = options.expected_fingerprint_sha256;
    let begin = solstone_core_brain::begin_prerequisite_renewal(
        &line.path,
        chrono::Utc::now(),
        options.run_id,
        expected_fingerprint_sha256.as_deref(),
        bundled_runtime_fingerprint_sha256.clone(),
    );
    let permit = match begin {
        solstone_core_brain::BeginPrerequisiteRenewal::Started(permit) => permit,
        solstone_core_brain::BeginPrerequisiteRenewal::Busy { reason } => {
            return write_session_result(json!({
                "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
                "kind": "not_started",
                "status": "busy",
                "reason": reason,
            }));
        }
        solstone_core_brain::BeginPrerequisiteRenewal::Unsafe { reason } => {
            return write_session_result(json!({
                "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
                "kind": "not_started",
                "status": "unsafe",
                "reason": reason,
            }));
        }
    };
    run_prerequisite_renewal_session_loop(
        line.path,
        permit,
        bundled_runtime_fingerprint_sha256,
        session_input_timeout(),
    )
}

fn brain_record_revision(journal_path: &std::path::Path) -> Option<u64> {
    fs::read(solstone_core_brain::brain_state_path(journal_path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|record| record.get("revision").and_then(Value::as_u64))
}

fn brain_record_was_written(before: Option<u64>, after: Option<u64>) -> bool {
    match (before, after) {
        (None, Some(_)) => true,
        (Some(before), Some(after)) => before != after,
        _ => false,
    }
}

fn session_input_timeout() -> Duration {
    // Integration binaries are debug builds, so this bounded test hook avoids
    // making AC15 wait ninety seconds. Release builds always use the protocol
    // contract's fixed ninety-second bound.
    #[cfg(debug_assertions)]
    if let Some(timeout) = env::var("SOLSTONE_CORE_BRAIN_SESSION_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|timeout| *timeout > 0)
    {
        return Duration::from_millis(timeout);
    }
    SESSION_INPUT_TIMEOUT
}

fn run_refresh_session_loop(
    journal_path: PathBuf,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
    timeout: Duration,
) -> ExitCode {
    if write_session_result(json!({"schema": REFRESH_READY_SCHEMA})) != ExitCode::SUCCESS {
        return ExitCode::from(EXIT_IOERR);
    }
    let deadline = Instant::now() + timeout;
    match read_refresh_session_input(deadline) {
        RefreshSessionInput::Clean(outcome) => match solstone_core_brain::finish_refresh(
            &journal_path,
            permit,
            outcome,
            chrono::Utc::now(),
            bundled_runtime_fingerprint_sha256,
        ) {
            Ok(_) => write_refresh_projection(&journal_path),
            Err(error) => {
                eprintln!("brain refresh failed: {error}");
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        RefreshSessionInput::CallerAbandon(abandon) => {
            abandon_refresh_for_caller(&journal_path, permit, abandon)
        }
        RefreshSessionInput::BareEof => {
            abandon_refresh_silently(&journal_path, permit);
            // A bare EOF means no caller remains to observe an answer.
            ExitCode::from(EXIT_UNAVAILABLE)
        }
        RefreshSessionInput::Timeout => {
            abandon_refresh(&journal_path, permit, true);
            ExitCode::SUCCESS
        }
        RefreshSessionInput::ProtocolViolation => {
            abandon_refresh(&journal_path, permit, true);
            ExitCode::from(EXIT_PROTOCOL)
        }
    }
}

fn run_prerequisite_renewal_session_loop(
    journal_path: PathBuf,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
    timeout: Duration,
) -> ExitCode {
    if write_session_result(json!({"schema": PREREQUISITE_RENEWAL_READY_SCHEMA}))
        != ExitCode::SUCCESS
    {
        return ExitCode::from(EXIT_IOERR);
    }
    let deadline = Instant::now() + timeout;
    match read_prerequisite_renewal_session_input(deadline) {
        PrerequisiteRenewalSessionInput::Clean(component) => {
            match solstone_core_brain::finish_prerequisite_renewal(
                &journal_path,
                permit,
                component,
                chrono::Utc::now(),
                bundled_runtime_fingerprint_sha256,
            ) {
                Ok(_) => write_brain_projection(&journal_path, PREREQUISITE_RENEWAL_RESULT_SCHEMA),
                Err(error) => {
                    eprintln!("brain prerequisite renewal failed: {error}");
                    ExitCode::from(EXIT_UNAVAILABLE)
                }
            }
        }
        PrerequisiteRenewalSessionInput::CallerAbandon(abandon) => {
            abandon_prerequisite_renewal_for_caller(
                &journal_path,
                permit,
                abandon,
                bundled_runtime_fingerprint_sha256,
            )
        }
        PrerequisiteRenewalSessionInput::BareEof => {
            abandon_prerequisite_renewal_silently(
                &journal_path,
                permit,
                bundled_runtime_fingerprint_sha256,
            );
            // A bare EOF means no caller remains to observe an answer.
            ExitCode::from(EXIT_UNAVAILABLE)
        }
        PrerequisiteRenewalSessionInput::Timeout => {
            abandon_prerequisite_renewal(
                &journal_path,
                permit,
                bundled_runtime_fingerprint_sha256,
                true,
            );
            ExitCode::SUCCESS
        }
        PrerequisiteRenewalSessionInput::ProtocolViolation => {
            abandon_prerequisite_renewal(
                &journal_path,
                permit,
                bundled_runtime_fingerprint_sha256,
                true,
            );
            ExitCode::from(EXIT_PROTOCOL)
        }
    }
}

enum RefreshSessionInput {
    Clean(Value),
    CallerAbandon(CallerAbandon),
    BareEof,
    Timeout,
    ProtocolViolation,
}

enum PrerequisiteRenewalSessionInput {
    Clean(Value),
    CallerAbandon(CallerAbandon),
    BareEof,
    Timeout,
    ProtocolViolation,
}

struct CallerAbandon {
    reason_code: String,
    diagnostic: Map<String, Value>,
}

enum CappedLineEvent {
    Line { bytes: Vec<u8>, count: usize },
    IoError,
}

fn spawn_capped_line_reader(max_bytes: usize) -> mpsc::Receiver<CappedLineEvent> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            let mut bytes = Vec::new();
            let read = {
                let mut limited = reader.by_ref().take(max_bytes as u64);
                limited.read_until(b'\n', &mut bytes)
            };
            match read {
                Ok(count) => {
                    if sender.send(CappedLineEvent::Line { bytes, count }).is_err() || count == 0 {
                        return;
                    }
                }
                Err(_) => {
                    let _ = sender.send(CappedLineEvent::IoError);
                    return;
                }
            }
        }
    });
    receiver
}

fn read_refresh_session_input(deadline: Instant) -> RefreshSessionInput {
    let receiver = spawn_capped_line_reader(MAX_JSON_STDIN_BYTES + 1);
    let mut request = None;
    let mut terminal = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return RefreshSessionInput::Timeout;
        }
        let (line, count) = match receiver.recv_timeout(remaining) {
            Ok(CappedLineEvent::Line { bytes, count }) => (bytes, count),
            Ok(CappedLineEvent::IoError) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return RefreshSessionInput::ProtocolViolation;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return RefreshSessionInput::Timeout,
        };
        if count == 0 {
            return if terminal {
                match request.expect("terminal requires request") {
                    RefreshSessionRecord::Probe(outcome) => RefreshSessionInput::Clean(outcome),
                    RefreshSessionRecord::Abandon(abandon) => {
                        RefreshSessionInput::CallerAbandon(abandon)
                    }
                    RefreshSessionRecord::Terminal => unreachable!("terminal is not buffered"),
                }
            } else {
                RefreshSessionInput::BareEof
            };
        }
        if line.len() > MAX_JSON_STDIN_BYTES {
            return RefreshSessionInput::ProtocolViolation;
        }
        match parse_refresh_session_record(&line, chrono::Utc::now()) {
            Some(record @ (RefreshSessionRecord::Probe(_) | RefreshSessionRecord::Abandon(_)))
                if !terminal && request.is_none() =>
            {
                request = Some(record);
            }
            Some(RefreshSessionRecord::Terminal) if request.is_some() => {
                terminal = true;
            }
            _ => return RefreshSessionInput::ProtocolViolation,
        }
    }
}

fn read_prerequisite_renewal_session_input(deadline: Instant) -> PrerequisiteRenewalSessionInput {
    let receiver = spawn_capped_line_reader(MAX_JSON_STDIN_BYTES + 1);
    let mut request = None;
    let mut terminal = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return PrerequisiteRenewalSessionInput::Timeout;
        }
        let (line, count) = match receiver.recv_timeout(remaining) {
            Ok(CappedLineEvent::Line { bytes, count }) => (bytes, count),
            Ok(CappedLineEvent::IoError) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return PrerequisiteRenewalSessionInput::ProtocolViolation;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return PrerequisiteRenewalSessionInput::Timeout;
            }
        };
        if count == 0 {
            return if terminal {
                match request.expect("terminal requires request") {
                    PrerequisiteRenewalSessionRecord::Probe(component) => {
                        PrerequisiteRenewalSessionInput::Clean(component)
                    }
                    PrerequisiteRenewalSessionRecord::Abandon(abandon) => {
                        PrerequisiteRenewalSessionInput::CallerAbandon(abandon)
                    }
                    PrerequisiteRenewalSessionRecord::Terminal => {
                        unreachable!("terminal is not buffered")
                    }
                }
            } else {
                PrerequisiteRenewalSessionInput::BareEof
            };
        }
        if line.len() > MAX_JSON_STDIN_BYTES {
            return PrerequisiteRenewalSessionInput::ProtocolViolation;
        }
        match parse_prerequisite_renewal_session_record(&line, chrono::Utc::now()) {
            Some(
                record @ (PrerequisiteRenewalSessionRecord::Probe(_)
                | PrerequisiteRenewalSessionRecord::Abandon(_)),
            ) if !terminal && request.is_none() => {
                request = Some(record);
            }
            Some(PrerequisiteRenewalSessionRecord::Terminal) if request.is_some() => {
                terminal = true;
            }
            _ => return PrerequisiteRenewalSessionInput::ProtocolViolation,
        }
    }
}

enum RefreshSessionRecord {
    Probe(Value),
    Abandon(CallerAbandon),
    Terminal,
}

enum PrerequisiteRenewalSessionRecord {
    Probe(Value),
    Abandon(CallerAbandon),
    Terminal,
}

fn parse_refresh_session_record(
    bytes: &[u8],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<RefreshSessionRecord> {
    let object = serde_json::from_slice::<Value>(bytes)
        .ok()?
        .as_object()?
        .clone();
    let schema = object.get("schema")?.as_str()?;
    if schema == REFRESH_TERMINAL_SCHEMA && exact_fields(&object, &["schema"]) {
        return Some(RefreshSessionRecord::Terminal);
    }
    if schema == REFRESH_ABANDON_SCHEMA {
        return parse_caller_abandon(&object).map(RefreshSessionRecord::Abandon);
    }
    if schema != REFRESH_PROBE_SCHEMA || !exact_fields(&object, &["schema", "outcome"]) {
        return None;
    }
    let outcome = object.get("outcome")?.clone();
    solstone_core_brain::validate_refresh_probe_outcome(&outcome, now).ok()?;
    Some(RefreshSessionRecord::Probe(outcome))
}

fn parse_prerequisite_renewal_session_record(
    bytes: &[u8],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<PrerequisiteRenewalSessionRecord> {
    let object = serde_json::from_slice::<Value>(bytes)
        .ok()?
        .as_object()?
        .clone();
    let schema = object.get("schema")?.as_str()?;
    if schema == PREREQUISITE_RENEWAL_TERMINAL_SCHEMA && exact_fields(&object, &["schema"]) {
        return Some(PrerequisiteRenewalSessionRecord::Terminal);
    }
    if schema == PREREQUISITE_RENEWAL_ABANDON_SCHEMA {
        return parse_caller_abandon(&object).map(PrerequisiteRenewalSessionRecord::Abandon);
    }
    if schema != PREREQUISITE_RENEWAL_PROBE_SCHEMA
        || !exact_fields(&object, &["schema", "lane_prerequisites"])
    {
        return None;
    }
    let component = object.get("lane_prerequisites")?.clone();
    let evidence = json!({
        "configuration": null,
        "lane_prerequisites": component,
        "generate": null,
        "cogitate": null,
    });
    solstone_core_brain::validate_refresh_probe_outcome(&evidence, now).ok()?;
    Some(PrerequisiteRenewalSessionRecord::Probe(
        evidence["lane_prerequisites"].clone(),
    ))
}

fn parse_caller_abandon(object: &Map<String, Value>) -> Option<CallerAbandon> {
    if !exact_fields(object, &["schema", "reason_code"])
        && !exact_fields(object, &["schema", "reason_code", "diagnostic"])
    {
        return None;
    }
    let reason_code = object.get("reason_code")?.as_str()?;
    if reason_code.is_empty() {
        return None;
    }
    let diagnostic = match object.get("diagnostic") {
        None => Map::new(),
        Some(Value::Object(diagnostic)) => diagnostic.clone(),
        Some(_) => return None,
    };
    Some(CallerAbandon {
        reason_code: reason_code.to_owned(),
        diagnostic,
    })
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|field| object.contains_key(*field))
}

fn abandon_refresh_silently(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
) {
    if let Err(error) = solstone_core_brain::abandon_refresh(
        journal_path,
        permit,
        "brain_refresh_timeout",
        Map::new(),
        chrono::Utc::now(),
    ) {
        eprintln!("brain refresh abandonment failed: {error}");
    }
}

fn abandon_refresh(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    report: bool,
) {
    abandon_refresh_silently(journal_path, permit);
    if report
        && write_session_result(json!({
            "schema": REFRESH_RESULT_SCHEMA,
            "kind": "abandoned",
            "reason_code": "brain_refresh_timeout",
            "component": "generate",
        })) != ExitCode::SUCCESS
    {
        eprintln!("brain refresh abandonment report failed: stdout I/O error");
    }
}

fn abandon_refresh_for_caller(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    abandon: CallerAbandon,
) -> ExitCode {
    let before = read_brain_record_value(journal_path);
    let reason_code = abandon.reason_code;
    match solstone_core_brain::abandon_refresh(
        journal_path,
        permit,
        &reason_code,
        abandon.diagnostic,
        chrono::Utc::now(),
    ) {
        Ok(record) => match changed_evidence_component(&before, &record, &reason_code) {
            Some(component) => write_session_result(json!({
                "schema": REFRESH_RESULT_SCHEMA,
                "kind": "abandoned",
                "reason_code": reason_code,
                "component": component,
            })),
            None => {
                eprintln!("brain refresh abandonment failed: could not identify changed component");
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        Err(error) => {
            eprintln!("brain refresh abandonment failed: {error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}

fn write_refresh_projection(journal_path: &std::path::Path) -> ExitCode {
    write_brain_projection(journal_path, REFRESH_RESULT_SCHEMA)
}

fn abandon_prerequisite_renewal_silently(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
) {
    if let Err(error) = solstone_core_brain::abandon_prerequisite_renewal(
        journal_path,
        permit,
        "nvattest_unavailable",
        Map::new(),
        chrono::Utc::now(),
        bundled_runtime_fingerprint_sha256,
    ) {
        eprintln!("brain prerequisite renewal abandonment failed: {error}");
    }
}

fn abandon_prerequisite_renewal(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    bundled_runtime_fingerprint_sha256: Option<String>,
    report: bool,
) {
    abandon_prerequisite_renewal_silently(journal_path, permit, bundled_runtime_fingerprint_sha256);
    if report
        && write_session_result(json!({
            "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
            "kind": "abandoned",
            "reason_code": "nvattest_unavailable",
            "component": "lane_prerequisites",
        })) != ExitCode::SUCCESS
    {
        eprintln!("brain prerequisite renewal abandonment report failed: stdout I/O error");
    }
}

fn abandon_prerequisite_renewal_for_caller(
    journal_path: &std::path::Path,
    permit: solstone_core_brain::BrainRefreshPermit,
    abandon: CallerAbandon,
    bundled_runtime_fingerprint_sha256: Option<String>,
) -> ExitCode {
    let before = read_brain_record_value(journal_path);
    let reason_code = abandon.reason_code;
    match solstone_core_brain::abandon_prerequisite_renewal(
        journal_path,
        permit,
        &reason_code,
        abandon.diagnostic,
        chrono::Utc::now(),
        bundled_runtime_fingerprint_sha256,
    ) {
        Ok(record) => match changed_evidence_component(&before, &record, &reason_code) {
            Some(component) => write_session_result(json!({
                "schema": PREREQUISITE_RENEWAL_RESULT_SCHEMA,
                "kind": "abandoned",
                "reason_code": reason_code,
                "component": component,
            })),
            None => {
                eprintln!(
                    "brain prerequisite renewal abandonment failed: could not identify changed component"
                );
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        Err(error) => {
            eprintln!("brain prerequisite renewal abandonment failed: {error}");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}

fn read_brain_record_value(journal_path: &std::path::Path) -> Option<Value> {
    fs::read(solstone_core_brain::brain_state_path(journal_path))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn changed_evidence_component(
    before: &Option<Value>,
    after: &Value,
    reason_code: &str,
) -> Option<String> {
    let before_evidence = before
        .as_ref()
        .and_then(|record| record.get("evidence"))
        .and_then(Value::as_object);
    after
        .get("evidence")
        .and_then(Value::as_object)?
        .iter()
        .find_map(|(component, value)| {
            (before_evidence.and_then(|evidence| evidence.get(component)) != Some(value)
                && value.get("reason_code").and_then(Value::as_str) == Some(reason_code))
            .then(|| component.clone())
        })
}

fn write_brain_projection(journal_path: &std::path::Path, result_schema: &str) -> ExitCode {
    let config = match read_journal_config(journal_path) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("brain refresh failed: could not read journal config: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let projection =
        solstone_core_brain::inspect_brain_state(journal_path, &config, chrono::Utc::now())
            .projection;
    write_session_result(json!({
        "schema": result_schema,
        "kind": "projection",
        "projection": {
            "aggregate_state": projection.aggregate_state,
            "reason_code": projection.reason_code,
            "active_lane": projection.active_lane,
            "active_provider": projection.active_provider,
            "active_model": projection.active_model,
            "fingerprint_sha256": projection.fingerprint_sha256,
            "runtime_transition_in_progress": projection.runtime_transition_in_progress,
        },
    }))
}

fn write_session_result(output: Value) -> ExitCode {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        let written = serde_json::to_writer(&mut stdout, &output).is_ok()
            && stdout.write_all(b"\n").is_ok()
            && stdout.flush().is_ok();
        let _ = sender.send(written);
    });
    if receiver
        .recv_timeout(SESSION_RESULT_WRITE_TIMEOUT)
        .is_ok_and(|written| written)
    {
        ExitCode::SUCCESS
    } else {
        eprintln!("brain refresh failed: stdout I/O error");
        ExitCode::from(EXIT_IOERR)
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
        None | Some(Value::Null) => None,
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
    let value = read_bounded_json_value(MAX_JSON_STDIN_BYTES)?;
    value.as_object().cloned().ok_or(JsonStdinError::Content)
}

fn read_bounded_json_value(max_bytes: usize) -> Result<Value, JsonStdinError> {
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
            .is_none_or(|next| next > max_bytes)
        {
            return Err(JsonStdinError::Content);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    serde_json::from_slice::<Value>(&bytes).map_err(|_| JsonStdinError::Content)
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

fn run_mcp_process(command: McpCommand) -> ExitCode {
    match command {
        McpCommand::Service => run_mcp_service(),
        McpCommand::Status => run_mcp_status(),
        McpCommand::Token(command) => run_mcp_token(command),
        McpCommand::Pairing(command) => run_mcp_pairing(command),
        McpCommand::Oauth(command) => run_mcp_oauth(command),
    }
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn run_mcp_service() -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.path.clone();
    run_hosted_service(&journal.path, HostedServiceKind::Mcp, move |parent| {
        match solstone_core::run_native_service_with_hosted_parent(service_journal, parent) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("mcp service failed: {}", error.class());
                ExitCode::from(EXIT_TEMPFAIL)
            }
        }
    })
}

#[cfg(not(all(unix, feature = "journal-mcp-endpoint")))]
fn run_mcp_service() -> ExitCode {
    eprintln!("journal mcp service is not compiled into this build");
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn run_mcp_status() -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };

    let mut exit = ExitCode::SUCCESS;
    let capability = match read_journal_config(&journal.path) {
        Ok(read) => match mcp_endpoint_capability(&read) {
            Ok(McpEndpointCapability::Enabled) => "enabled".to_owned(),
            Ok(McpEndpointCapability::Disabled) => "disabled".to_owned(),
            Err(error) => {
                eprintln!("journal mcp status: MCP capability configuration is invalid: {error:?}");
                exit = ExitCode::from(EXIT_UNAVAILABLE);
                "error".to_owned()
            }
        },
        Err(error) => {
            eprintln!("journal mcp status: could not read MCP capability configuration: {error}");
            exit = ExitCode::from(EXIT_UNAVAILABLE);
            "error".to_owned()
        }
    };

    let token_count = match TokenStore::open(&journal.path).list() {
        Ok(tokens) => tokens.len().to_string(),
        Err(error) => {
            eprintln!("journal mcp status: could not read bearer-token ledger: {error}");
            exit = ExitCode::from(token_store_error_exit(&error));
            "unavailable".to_owned()
        }
    };

    println!("MCP capability/config status only; this does not report service liveness.");
    println!("capability compiled in: true");
    println!("capability: {capability}");
    println!("token count: {token_count}");
    exit
}

#[cfg(not(all(unix, feature = "journal-mcp-endpoint")))]
fn run_mcp_status() -> ExitCode {
    eprintln!("journal mcp status is not compiled into this build");
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn run_mcp_token(command: McpTokenCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let store = TokenStore::open(&journal.path);

    match command {
        McpTokenCommand::Create { label } => match store.create(&label) {
            Ok(created) => {
                println!("Created bearer token for label {:?}.", created.label);
                println!(
                    "Save this token now. It will not be shown again and cannot be recovered:"
                );
                println!("{}", created.token);
                ExitCode::SUCCESS
            }
            Err(error) => render_token_store_error("create", &error),
        },
        McpTokenCommand::List => match store.list() {
            Ok(tokens) => {
                for token in tokens {
                    println!("{}\t{}", token.label, token.created_at.to_rfc3339());
                }
                ExitCode::SUCCESS
            }
            Err(error) => render_token_store_error("list", &error),
        },
        McpTokenCommand::Revoke { label } => match store.revoke(&label) {
            Ok(()) => {
                println!("Revoked bearer token for label {label:?}.");
                ExitCode::SUCCESS
            }
            Err(error) => render_token_store_error("revoke", &error),
        },
    }
}

#[cfg(not(all(unix, feature = "journal-mcp-endpoint")))]
fn run_mcp_token(_command: McpTokenCommand) -> ExitCode {
    eprintln!("journal mcp token management is not compiled into this build");
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn run_mcp_pairing(command: McpPairingCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let store = OAuthStore::open(&journal.path);
    match command {
        McpPairingCommand::Generate => match store.generate_pairing_code() {
            Ok(created) => {
                println!("Generated a pairing code, valid for 10 minutes and one use.");
                println!("Save this now. It will not be shown again and cannot be recovered:");
                println!("{}", created.code);
                println!("Expires at {}.", created.expires_at.to_rfc3339());
                ExitCode::SUCCESS
            }
            Err(error) => render_oauth_store_error("pairing", "generate", &error),
        },
        McpPairingCommand::Revoke => match store.revoke_pairing_code() {
            Ok(()) => {
                println!("Revoked the active pairing code.");
                ExitCode::SUCCESS
            }
            Err(OAuthStoreError::NoActivePairing) => {
                eprintln!("journal mcp pairing revoke: no active pairing code to revoke");
                ExitCode::from(EXIT_DATAERR)
            }
            Err(error) => render_oauth_store_error("pairing", "revoke", &error),
        },
    }
}

#[cfg(not(all(unix, feature = "journal-mcp-endpoint")))]
fn run_mcp_pairing(_command: McpPairingCommand) -> ExitCode {
    eprintln!("journal mcp pairing is not compiled into this build");
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn run_mcp_oauth(command: McpOauthCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let store = OAuthStore::open(&journal.path);
    match command {
        McpOauthCommand::List => match store.list_clients() {
            Ok(clients) => {
                for client in clients {
                    let name = client.client_name.as_deref().unwrap_or("-");
                    println!(
                        "{}\t{}\t{}",
                        client.client_id,
                        name,
                        client.created_at.to_rfc3339()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => render_oauth_store_error("oauth", "list", &error),
        },
        McpOauthCommand::Revoke { client_id } => match store.revoke_client_by_client_id(&client_id)
        {
            Ok(()) => {
                println!("Revoked OAuth client {client_id}.");
                ExitCode::SUCCESS
            }
            Err(OAuthStoreError::ClientNotFound) => {
                eprintln!("journal mcp oauth revoke: no such OAuth client");
                ExitCode::from(EXIT_DATAERR)
            }
            Err(error) => render_oauth_store_error("oauth", "revoke", &error),
        },
    }
}

#[cfg(not(all(unix, feature = "journal-mcp-endpoint")))]
fn run_mcp_oauth(_command: McpOauthCommand) -> ExitCode {
    eprintln!("journal mcp oauth is not compiled into this build");
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn render_token_store_error(operation: &str, error: &TokenStoreError) -> ExitCode {
    match error {
        TokenStoreError::InvalidLabel(error) => {
            eprintln!("journal mcp token {operation}: invalid label: {error}");
        }
        TokenStoreError::DuplicateLabel { label } => {
            eprintln!(
                "journal mcp token {operation}: a bearer token already exists for label {label:?}"
            );
        }
        TokenStoreError::NotFound { label } => {
            eprintln!("journal mcp token {operation}: no bearer token exists for label {label:?}");
        }
        _ => {
            eprintln!("journal mcp token {operation}: {error}");
        }
    }
    ExitCode::from(token_store_error_exit(error))
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
const fn token_store_error_exit(error: &TokenStoreError) -> u8 {
    match error {
        TokenStoreError::InvalidLabel(_) | TokenStoreError::DuplicateLabel { .. } => EXIT_USAGE,
        TokenStoreError::NotFound { .. } | TokenStoreError::InvalidToken => EXIT_DATAERR,
        TokenStoreError::Lock(_) => EXIT_TEMPFAIL,
        TokenStoreError::Directory(_) | TokenStoreError::Read { .. } => EXIT_IOERR,
        TokenStoreError::Write(_) => EXIT_CANTCREAT,
        TokenStoreError::Randomness
        | TokenStoreError::Malformed { .. }
        | TokenStoreError::UnsupportedSchema { .. } => EXIT_UNAVAILABLE,
    }
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn render_oauth_store_error(command: &str, operation: &str, error: &OAuthStoreError) -> ExitCode {
    match error {
        OAuthStoreError::NoActivePairing => {
            eprintln!("journal mcp {command} {operation}: no active pairing code");
        }
        OAuthStoreError::ClientNotFound => {
            eprintln!("journal mcp {command} {operation}: no such OAuth client");
        }
        _ => {
            eprintln!("journal mcp {command} {operation}: {error}");
        }
    }
    ExitCode::from(oauth_store_error_exit(error))
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
const fn oauth_store_error_exit(error: &OAuthStoreError) -> u8 {
    match error {
        OAuthStoreError::NoActivePairing
        | OAuthStoreError::ClientNotFound
        | OAuthStoreError::InvalidToken
        | OAuthStoreError::TransactionNotFound
        | OAuthStoreError::TransactionExpired
        | OAuthStoreError::TransactionExhausted
        | OAuthStoreError::CodeExpired
        | OAuthStoreError::BindingMismatch
        | OAuthStoreError::PairingMismatch
        | OAuthStoreError::PairingLocked => EXIT_DATAERR,
        OAuthStoreError::Lock(_) => EXIT_TEMPFAIL,
        OAuthStoreError::Directory(_) | OAuthStoreError::Read { .. } => EXIT_IOERR,
        OAuthStoreError::Write(_) => EXIT_CANTCREAT,
        OAuthStoreError::Randomness
        | OAuthStoreError::Quota
        | OAuthStoreError::Malformed { .. }
        | OAuthStoreError::UnsupportedSchema { .. }
        | OAuthStoreError::EntryTooLarge
        | OAuthStoreError::StateTooLarge => EXIT_UNAVAILABLE,
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
    let service_journal = journal.path.clone();
    run_hosted_service(&journal.path, HostedServiceKind::Spl, move |parent| {
        let verbosity = solstone_core_spl::Verbosity::from_flags(options.verbose, options.debug);
        match solstone_core_spl::run_native_service_with_hosted_parent(
            service_journal,
            verbosity,
            parent,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("spl service failed: {}", error.class());
                ExitCode::from(EXIT_TEMPFAIL)
            }
        }
    })
}

fn run_sense_service(options: ServiceOptions) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    // Borrows the hosting supervisor's inherited generation when present
    // (the ordinary hosted case); otherwise acquires as its own root owner
    // (an unhosted `journal sense` daemon run outside the supervisor).
    let generation = match solstone_core_transcribe::enter_speakers_analyze_generation(
        &journal.path,
        solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Sense,
    ) {
        Ok(generation) => generation,
        Err(error) => {
            if let Some(message) = error.message() {
                eprintln!("{message}");
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let sense_child_environment = generation.inheritance_environment();
    let service_journal = journal.path.clone();
    run_hosted_service(&journal.path, HostedServiceKind::Sense, move |parent| {
        let _generation = generation;
        match solstone_core_sense::run_native_service_with_hosted_parent(
            service_journal,
            solstone_core_sense::SenseOptions {
                verbose: options.verbose,
                debug: options.debug,
            },
            sense_child_environment,
            parent,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("sense service failed: {}", error.class());
                ExitCode::from(EXIT_TEMPFAIL)
            }
        }
    })
}

fn run_cortex_service(options: ServiceOptions) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.path.clone();
    run_hosted_service(&journal.path, HostedServiceKind::Cortex, move |parent| {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => return ExitCode::from(EXIT_TEMPFAIL),
        };
        match runtime.block_on(solstone_core_cortex::run_native_service_with_hosted_parent(
            service_journal,
            solstone_core_cortex::CortexOptions {
                verbose: options.verbose,
                debug: options.debug,
            },
            parent,
        )) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("cortex service failed: {error}");
                ExitCode::from(EXIT_TEMPFAIL)
            }
        }
    })
}

fn run_sense(options: SenseOptions) -> ExitCode {
    let Some(day) = options.day.clone() else {
        return run_sense_service(ServiceOptions {
            verbose: options.verbose,
            debug: options.debug,
        });
    };
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    if let Err(error) = solstone_core_transcribe::require_solstone(&journal.path) {
        if let Some(message) = error.message() {
            eprintln!("{message}");
        }
        return ExitCode::from(error.exit_code() as u8);
    }
    let _generation = match solstone_core_transcribe::enter_speakers_analyze_generation(
        &journal.path,
        solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Sense,
    ) {
        Ok(generation) => generation,
        Err(error) => {
            if let Some(message) = error.message() {
                eprintln!("{message}");
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    solstone_core_sense::batch::install_batch_signal_handlers();
    let reprocess = options.reprocess.map(|kind| match kind {
        SenseReprocessKind::Screen => solstone_core_sense::batch::ReprocessKind::Screen,
        SenseReprocessKind::Audio => solstone_core_sense::batch::ReprocessKind::Audio,
        SenseReprocessKind::All => solstone_core_sense::batch::ReprocessKind::All,
    });
    let request = solstone_core_sense::batch::BatchRequest {
        day,
        jobs: options.jobs,
        reprocess,
        segment: options.segment,
        stream: options.stream,
        dry_run: options.dry_run,
        verbose: options.verbose,
        debug: options.debug,
    };
    match solstone_core_sense::batch::run_batch_with_environment(
        &journal.path,
        &request,
        &_generation.inheritance_environment(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sense batch failed: {error}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
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

fn run_body(command: BodyCommand) -> ExitCode {
    match command {
        BodyCommand::Rebuild(options) => run_body_rebuild(options),
        BodyCommand::Apple(options) => run_body_apple(options),
        BodyCommand::Oura(command) => run_body_oura(command),
    }
}

fn run_body_oura(command: BodyOuraCommand) -> ExitCode {
    match command {
        BodyOuraCommand::Connect(options) => run_body_oura_connect(options),
        BodyOuraCommand::Sync(options) => run_body_oura_sync(options),
    }
}

fn run_body_oura_connect(options: BodyOuraConnectOptions) -> ExitCode {
    use solstone_core_body_ingest::{BodyIngestErrorKind, OuraConnectOptions, connect_oura};

    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match connect_oura(&journal, &OuraConnectOptions::default()) {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "schema": "solstone.body.oura.connect.result.v1",
                    "connected": true,
                    "scopes": report.scopes(),
                }));
            } else {
                println!("Oura authorization saved to journal config.");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Oura authorization failed: {error}");
            ExitCode::from(match error.kind() {
                BodyIngestErrorKind::Gate => 2,
                BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
            })
        }
    }
}

fn run_body_oura_sync(options: BodyOuraSyncOptions) -> ExitCode {
    use solstone_core_body_ingest::{BodyIngestErrorKind, OuraSyncOptions, sync_oura};

    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let result = sync_oura(
        &journal,
        &OuraSyncOptions {
            save: options.save,
            confirm_body_save: options.confirm_body_save,
            scheduled: options.scheduled,
            window_days: options.window_days,
            today: None,
        },
    );
    match result {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "schema": "solstone.body.ingest.result.v1",
                    "source": "oura_api",
                    "mode": if options.save { "save" } else { "preview" },
                    "bundle_id": report.bundle_id(),
                    "rows": report.rows(),
                    "days": report.days(),
                    "pages": report.pages(),
                    "skipped": report.quiet_run(),
                    "endpoint_counts": report.endpoint_counts(),
                    "issues": report.issues().iter().map(|issue| json!({
                        "endpoint": issue.endpoint(),
                        "kind": issue.kind().as_str(),
                    })).collect::<Vec<_>>(),
                }));
            } else if options.save {
                println!(
                    "Oura body sync complete: rows={} days={} pages={} bundle={}",
                    report.rows(),
                    report.days().len(),
                    report.pages(),
                    report.bundle_id().unwrap_or("none")
                );
            } else {
                println!(
                    "Oura body sync preview: rows={} days={} pages={} (nothing written)",
                    report.rows(),
                    report.days().len(),
                    report.pages()
                );
            }
            if !options.json {
                for issue in report.issues() {
                    println!(
                        "Oura endpoint unavailable: {} ({})",
                        issue.endpoint(),
                        issue.kind().as_str()
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Oura body sync failed: {error}");
            ExitCode::from(match error.kind() {
                BodyIngestErrorKind::Gate => 2,
                BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
            })
        }
    }
}

fn run_body_apple(options: BodyAppleOptions) -> ExitCode {
    use solstone_core_body_ingest::{
        AppleImportOptions, BodyIngestErrorKind, detect_apple_source, preview_apple, save_apple,
    };

    let source = PathBuf::from(options.source);
    if options.detect {
        return match detect_apple_source(&source) {
            Ok(detected) => {
                if options.json {
                    print_json(&json!({
                        "schema": "solstone.body.apple.detect.result.v1",
                        "apple_health": detected,
                    }));
                } else {
                    println!(
                        "{}",
                        if detected {
                            "apple_health"
                        } else {
                            "not_apple_health"
                        }
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Apple Health source detection failed: {error}");
                ExitCode::from(match error.kind() {
                    BodyIngestErrorKind::Gate => 2,
                    BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                    BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
                })
            }
        };
    }
    let result = if options.save {
        let journal = match resolve_indexer_journal_path(options.journal_override) {
            Ok(line) => line.path,
            Err(error) => return print_journal_error(error),
        };
        save_apple(
            &source,
            &journal,
            &AppleImportOptions {
                date_from: options.date_from,
                date_to: options.date_to,
                confirm_body_save: options.confirm_body_save,
                force: options.force,
            },
        )
    } else {
        preview_apple(
            &source,
            options.date_from.as_deref(),
            options.date_to.as_deref(),
        )
    };
    match result {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "schema": "solstone.body.ingest.result.v1",
                    "source": "apple_health",
                    "mode": if options.save { "save" } else { "preview" },
                    "bundle_id": report.bundle_id(),
                    "rows": report.rows(),
                    "days": report.days(),
                    "skipped": report.skipped(),
                }));
            } else if options.save {
                println!(
                    "Apple Health body import complete: rows={} days={} bundle={}",
                    report.rows(),
                    report.days().len(),
                    report.bundle_id().unwrap_or("none")
                );
            } else {
                println!(
                    "Apple Health body import preview: rows={} days={} (nothing written)",
                    report.rows(),
                    report.days().len()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Apple Health body import failed: {error}");
            ExitCode::from(match error.kind() {
                BodyIngestErrorKind::Gate => 2,
                BodyIngestErrorKind::Source | BodyIngestErrorKind::Normalize => EXIT_DATAERR,
                BodyIngestErrorKind::Publication | BodyIngestErrorKind::Rebuild => EXIT_IOERR,
            })
        }
    }
}

fn run_body_rebuild(options: BodyRebuildOptions) -> ExitCode {
    let json_output = options.json;
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match solstone_core_body_rebuild::rebuild_body_store(&journal) {
        Ok(report) => {
            if json_output {
                print_json(&json!({
                    "schema": "solstone.body.rebuild.result.v1",
                    "native_bundles": report.native_bundles(),
                    "legacy_bundles": report.legacy_bundles(),
                    "rows": report.rows(),
                }));
            } else {
                println!(
                    "body rebuild complete: native_bundles={} legacy_bundles={} rows={}",
                    report.native_bundles(),
                    report.legacy_bundles(),
                    report.rows()
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("body rebuild failed: {error}");
            let exit = body_rebuild_error_exit(error.kind(), error.stage());
            ExitCode::from(exit)
        }
    }
}

fn body_rebuild_error_exit(
    kind: solstone_core_body_rebuild::BodyRebuildErrorKind,
    stage: &str,
) -> u8 {
    use solstone_core_body_rebuild::BodyRebuildErrorKind;

    match (kind, stage) {
        (BodyRebuildErrorKind::Publication, "database_lock_timeout") => EXIT_TEMPFAIL,
        (
            BodyRebuildErrorKind::Journal
            | BodyRebuildErrorKind::Sqlite
            | BodyRebuildErrorKind::Publication,
            _,
        ) => EXIT_IOERR,
        _ => EXIT_DATAERR,
    }
}

fn run_indexer(command: IndexerCommand) -> ExitCode {
    match command {
        IndexerCommand::Maintenance(options) => run_indexer_maintenance(options),
        IndexerCommand::Search(options) => run_indexer_search(options),
        IndexerCommand::Counts(options) => run_indexer_counts(options),
        IndexerCommand::Agents(options) => run_indexer_agents(options),
        IndexerCommand::Coverage(options) => run_indexer_coverage(options),
        IndexerCommand::PruneStream(options) => run_indexer_prune_stream(options),
        IndexerCommand::PrunePaths(options) => run_indexer_prune_paths(options),
        IndexerCommand::FoldEntityEdges(options) => run_indexer_fold_entity_edges(options),
        IndexerCommand::EdgeFingerprint(options) => run_indexer_edge_fingerprint(options),
        IndexerCommand::RebuildEdgesFingerprint(options) => {
            run_indexer_rebuild_edges_fingerprint(options)
        }
    }
}

fn run_indexer_maintenance(options: IndexerOptions) -> ExitCode {
    let json_output = options.json;
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
                if json_output && !options.rescan && !options.rescan_full {
                    print_json(&json!({
                        "files": report.files,
                        "rows": report.rows,
                        "drops": report.drops,
                        "failed": report.failed,
                        "skipped": report.skipped,
                    }));
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
                if json_output {
                    print_json(&json!({"indexed": true}));
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
                if should_emit_zero_edge_hint && !json_output {
                    println!("{ZERO_EDGE_HINT}");
                }
                if json_output {
                    print_json(&json!({
                        "indexed": report.indexed,
                        "removed": report.removed,
                        "skipped": report.skipped,
                        "edges_indexed": report.edges_indexed,
                        "edges_removed": report.edges_removed,
                        "edge_rows_inserted": report.edge_rows_inserted,
                        "merge_steps_spent": report.merge_steps_spent,
                        "failed": report.failed,
                    }));
                }
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("indexer scan failed: {error}");
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    }

    if json_output {
        print_json(&json!({"reset": options.reset}));
    }
    ExitCode::SUCCESS
}

fn run_indexer_prune_stream(options: IndexerPruneStreamOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match prune_chunks_by_stream(&journal, &options.stream) {
        Ok(counts) => {
            if options.json {
                print_json(&json!({"chunks": counts.chunks, "files": counts.files}));
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("prune-stream", error),
    }
}

fn run_indexer_prune_paths(options: IndexerPrunePathsOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let paths = options.paths.iter().map(String::as_str).collect::<Vec<_>>();
    match prune_by_paths(&journal, &paths) {
        Ok(counts) => {
            let counts = counts.unwrap_or_default();
            if options.json {
                print_json(&json!({"chunks": counts.chunks, "files": counts.files}));
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("prune-paths", error),
    }
}

fn run_indexer_fold_entity_edges(options: IndexerFoldEntityEdgesOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match fold_entity_edges_for_recorded_merge(&journal, &options.source_id, &options.target_id) {
        Ok(report) => {
            if options.json {
                print_json(&json!({
                    "rows_folded": report.rows_folded,
                    "self_edges_dropped": report.self_edges_dropped,
                    "fallback_rebuild": report.fallback_rebuild,
                }));
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("fold-entity-edges", error),
    }
}

fn run_indexer_edge_fingerprint(options: IndexerReadOptions) -> ExitCode {
    run_indexer_fingerprint(options, false)
}

fn run_indexer_rebuild_edges_fingerprint(options: IndexerReadOptions) -> ExitCode {
    run_indexer_fingerprint(options, true)
}

fn run_indexer_fingerprint(options: IndexerReadOptions, rebuild: bool) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let result = if rebuild {
        rebuild_edges_for_recorded_merge_undo(&journal)
    } else {
        fingerprint_edge_rows(&journal)
    };
    match result {
        Ok(fingerprint) => {
            if options.json {
                print_json(&json!({"fingerprint": fingerprint}));
            } else {
                println!("{fingerprint}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("edge-fingerprint", error),
    }
}

fn print_indexer_mutation_error(
    operation: &str,
    error: solstone_core_indexer_store::StoreError,
) -> ExitCode {
    eprintln!("indexer {operation} failed: {error}");
    ExitCode::from(EXIT_TEMPFAIL)
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
        _ => return Err(()),
    };
    let mut request = SearchRequest::new(options.query.clone().unwrap_or_default(), order);
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

// Packaged installs may not retain source-package roots and therefore report zero talent configs.
fn discover_package_roots() -> Option<(PathBuf, PathBuf)> {
    env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .and_then(|directory| crate::contract::paths::package_roots_from_executable_dir(&directory))
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

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

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
        assert_eq!(EXIT_PROTOCOL, 76);
    }

    #[test]
    fn resolve_declared_parent_skips_capture_when_hosting_is_disabled() {
        assert_eq!(resolve_declared_parent(false), Ok(None));
    }

    #[test]
    fn resolve_declared_parent_captures_the_live_direct_parent_when_hosting_is_enabled() {
        assert!(matches!(resolve_declared_parent(true), Ok(Some(_))));
    }

    #[test]
    fn parent_liveness_refusal_uses_the_existing_temporary_failure_renderer() {
        let outcome = supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::ParentLiveness(
                ParentAdmissionFailure::Unverifiable,
            ),
        };
        assert_eq!(
            render_supervisor_host_outcome(outcome),
            ExitCode::from(EXIT_TEMPFAIL)
        );
    }

    #[test]
    fn shutdown_cause_exit_codes_distinguish_conflict_from_operational_failure() {
        assert_eq!(
            exit_code_for_shutdown_cause(supervisor::ShutdownCause::Sync(
                supervisor::SyncFailureKind::Conflict
            )),
            2
        );
        for kind in [
            supervisor::SyncFailureKind::RenewalFailure,
            supervisor::SyncFailureKind::CompleteScanFailure,
            supervisor::SyncFailureKind::RetainedObservationFailure,
            supervisor::SyncFailureKind::StaleHeartbeatCollectionFailure,
        ] {
            assert_eq!(
                exit_code_for_shutdown_cause(supervisor::ShutdownCause::Sync(kind)),
                EXIT_TEMPFAIL
            );
        }
        assert_eq!(
            exit_code_for_shutdown_cause(supervisor::ShutdownCause::Signal(
                supervisor::SupervisorSignal::SigTerm
            )),
            0
        );
    }

    fn session_options(arguments: Vec<OsString>) -> GenerateSessionOptions {
        GenerateSessionOptions { arguments }
    }

    fn expected_session_arguments() -> String {
        let session = &solstone_core_generate::contract()["framing"]["session"];
        let selector = session["selector"].as_str().unwrap();
        let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
        let journal_flag = session["journal"]["flag"].as_str().unwrap();
        format!(
            "expected {selector} {concurrency_flag} <positive integer> [{journal_flag} <journal path>]"
        )
    }

    fn session_error(arguments: Vec<OsString>) -> String {
        match generate_session_config(&session_options(arguments)) {
            Ok(_) => panic!("generate session arguments unexpectedly parsed"),
            Err(error) => error,
        }
    }

    #[test]
    fn generate_session_config_accepts_optional_journal_in_either_order() {
        let session = &solstone_core_generate::contract()["framing"]["session"];
        let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
        let journal_flag = session["journal"]["flag"].as_str().unwrap();

        let absent =
            generate_session_config(&session_options(vec![concurrency_flag.into(), "2".into()]))
                .unwrap();
        assert_eq!(absent.session.max_in_flight, 2);
        assert_eq!(absent.explicit_journal, None);

        for arguments in [
            vec![
                journal_flag.into(),
                "journal-a".into(),
                concurrency_flag.into(),
                "3".into(),
            ],
            vec![
                concurrency_flag.into(),
                "3".into(),
                journal_flag.into(),
                "journal-a".into(),
            ],
        ] {
            let config = generate_session_config(&session_options(arguments)).unwrap();
            assert_eq!(config.session.max_in_flight, 3);
            assert_eq!(config.explicit_journal, Some(PathBuf::from("journal-a")));
        }
    }

    #[test]
    fn generate_session_config_rejects_incomplete_duplicate_and_unknown_pairs() {
        let session = &solstone_core_generate::contract()["framing"]["session"];
        let selector = session["selector"].as_str().unwrap();
        let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
        let journal_flag = session["journal"]["flag"].as_str().unwrap();
        let minimum = session["concurrency"]["minimum"].as_u64().unwrap();
        let expected = expected_session_arguments();

        for arguments in [
            vec![concurrency_flag.into(), "2".into(), journal_flag.into()],
            vec![concurrency_flag.into()],
            vec![journal_flag.into(), "journal-a".into()],
            vec!["--unknown".into(), "journal-a".into()],
        ] {
            assert_eq!(session_error(arguments), expected);
        }

        assert_eq!(
            session_error(vec![
                concurrency_flag.into(),
                "2".into(),
                concurrency_flag.into(),
                "2".into(),
            ]),
            format!("{concurrency_flag} was provided more than once for {selector}")
        );
        assert_eq!(
            session_error(vec![
                concurrency_flag.into(),
                "2".into(),
                journal_flag.into(),
                "journal-a".into(),
                journal_flag.into(),
                "journal-b".into(),
            ]),
            format!("{journal_flag} was provided more than once for {selector}")
        );
        assert_eq!(
            session_error(vec![concurrency_flag.into(), "0".into()]),
            format!("{concurrency_flag} is below the fixture minimum of {minimum} for {selector}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn generate_session_config_preserves_non_utf8_journal_path() {
        use std::os::unix::ffi::OsStringExt;

        let session = &solstone_core_generate::contract()["framing"]["session"];
        let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
        let journal_flag = session["journal"]["flag"].as_str().unwrap();
        let journal = OsString::from_vec(b"journal-\xff".to_vec());
        let config = generate_session_config(&session_options(vec![
            concurrency_flag.into(),
            "2".into(),
            journal_flag.into(),
            journal.clone(),
        ]))
        .unwrap();

        assert_eq!(config.explicit_journal, Some(PathBuf::from(journal)));
    }

    #[test]
    fn body_rebuild_errors_distinguish_data_io_and_retryable_lock_failure() {
        use solstone_core_body_rebuild::BodyRebuildErrorKind;

        assert_eq!(
            body_rebuild_error_exit(BodyRebuildErrorKind::Publication, "database_lock_timeout"),
            EXIT_TEMPFAIL
        );
        for (kind, stage) in [
            (BodyRebuildErrorKind::Journal, "journal_root"),
            (BodyRebuildErrorKind::Sqlite, "open_temp_database"),
            (BodyRebuildErrorKind::Publication, "database_lock"),
        ] {
            assert_eq!(body_rebuild_error_exit(kind, stage), EXIT_IOERR);
        }
        for kind in [
            BodyRebuildErrorKind::Authority,
            BodyRebuildErrorKind::Envelope,
            BodyRebuildErrorKind::NativeReplay,
            BodyRebuildErrorKind::LegacyReplay,
        ] {
            assert_eq!(body_rebuild_error_exit(kind, "semantic"), EXIT_DATAERR);
        }
    }

    #[test]
    fn generate_response_write_failure_is_reportable_as_internal_failure() {
        let mut writer = FailingWriter;
        assert!(write_generate_response_to(&mut writer, "{}").is_err());
    }

    #[test]
    fn brain_record_write_observation_requires_a_new_revision() {
        assert!(!brain_record_was_written(None, None));
        assert!(!brain_record_was_written(Some(7), Some(7)));
        assert!(brain_record_was_written(None, Some(1)));
        assert!(brain_record_was_written(Some(7), Some(8)));
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

    #[test]
    fn anthropic_generated_response_preserves_thinking_blocks() {
        let request = solstone_core_generate::GenerateRequest {
            id: Some("request".into()),
            context: "context".into(),
            contents: Vec::new(),
            system_instruction: None,
            temperature: 0.0,
            max_output_tokens: 1,
            thinking_budget: None,
            timeout_s: None,
            json_output: false,
            json_schema: None,
            enforce_responsiveness: false,
            attempt_index: 0,
            exclusive_admission: false,
            transport_retries: None,
        };
        let response = anthropic_generated_response(
            &request,
            solstone_core_generate_wire::AnthropicGenerated {
                text: "text".into(),
                model: "model".into(),
                usage: json!({"input_tokens": 1}),
                finish_reason: "stop".into(),
                thinking: Some(json!({"type": "thinking", "thinking": "work"})),
                raw_response_snippet: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            response.thinking,
            Some(json!({"type": "thinking", "thinking": "work"}))
        );
    }

    #[test]
    fn logger_install_is_idempotent_and_defaults_to_warn() {
        install_logger();
        install_logger();
        if std::env::var("RUST_LOG").is_err() {
            assert!(log::max_level() >= log::LevelFilter::Warn);
        }
    }
}
