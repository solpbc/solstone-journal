// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg_attr(not(windows), forbid(unsafe_code))]

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::ExitCode;
#[cfg(any(unix, windows))]
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use std::{
    env,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
#[cfg(all(test, feature = "full-tests"))]
use solstone_core_cli::evaluate_args;
use solstone_core_cli::{
    BrainCommand, BrainInspectOptions, BrainPrerequisiteRenewalSessionOptions,
    BrainRefreshExpectArg, BrainRefreshSessionOptions, BrainRuntimeFailureOptions, ChatGptCommand,
    Command, ContractCommand, ConveyOptions, GenerateCommand, GenerateSessionOptions, GrabCommand,
    GrabOptions, IndexerClassificationsOptions, IndexerCommand, IndexerCountsOptions,
    IndexerDrainOptions, IndexerOptions, IndexerPathLookupOptions, IndexerPrunePathsOptions,
    IndexerPruneStreamOptions, IndexerQueryOptions, IndexerReadOptions, IndexerSearchOptions,
    InstallCommand, JournalBrainOwnerCommand, JournalConfigCommand, JournalConfigCommitOptions,
    JournalConfigExpectArg, JournalConfigReadOptions, JournalPathOptions, LocalCommand, McpCommand,
    McpOauthCommand, McpPairingCommand, McpPermissionCommand, McpTokenCommand, ScheduleOptions,
    SenseOptions, SenseReprocessKind, ServiceAction, ServiceOptions, ServiceParseOutcome,
    SettingsParseError, SpeakerResolveCommand, SplCommand, ThinkingCommand, TranscribeOptions,
    USAGE, render_service_diagnostic, version_line,
};
use solstone_core_cli_copy::Value as CopyValue;
use solstone_core_transcribe::{CliError, CliRunError};
#[cfg(any(unix, windows))]
mod brain_owner;
mod check;
mod config;
mod contract;
mod facet_candidates;
mod health;
mod health_logs;
#[cfg(any(unix, windows))]
mod heartbeat;
#[cfg(windows)]
mod heartbeat_pid_windows;
mod importer_desktop;
mod install_models;
mod install_provider;
#[cfg(unix)]
mod journal_route;
mod navigate;
mod open;
#[cfg(unix)]
mod service;
#[cfg(unix)]
mod service_capture;
#[cfg(windows)]
mod service_capture_windows;
mod service_logs;
#[cfg(windows)]
mod service_windows;
mod settings;
#[cfg(any(unix, windows))]
use solstone_core::supervisor;
#[cfg(feature = "journal-mcp-endpoint")]
use solstone_core::{OAuthStore, OAuthStoreError, TokenStore, TokenStoreError};
#[cfg(feature = "journal-mcp-endpoint")]
use solstone_core_cli::McpTarget;
#[cfg(unix)]
use solstone_core_system::lifecycle::{
    CoordinatorBootstrap, ParentLossCoordinator, acknowledge_hosted_child_admission,
    arm_parent_loss_coordinator_termination_guard,
};
#[cfg(any(unix, windows))]
use solstone_core_system::lifecycle::{
    DeclaredParent, HostedServiceKind, HostedServiceParentRuntime, ParentAdmissionFailure,
    admit_hosted_service_parent,
};
#[cfg(unix)]
use solstone_core_system::process::ProcessInstance;
mod thinking;
use solstone_core_indexer_query::{
    IndexAccessError, Order, OwnerBoundary, QueryBoundary, SearchRequest, agents, coverage, search,
    search_counts,
};
use solstone_core_indexer_store::db::{prune_by_paths, prune_chunks_by_stream, reset_index};
use solstone_core_indexer_store::merge::fingerprint_edge_rows;
use solstone_core_indexer_store::scan::{
    RescanFileStatus, rebuild_edges, rescan_file, scan_journal,
};
use solstone_core_indexer_store::{
    apply_classification_batch, apply_path_lookup, drain_classifications, inspect_classifications,
    inspect_path_lookup,
};
use solstone_core_journal::{
    ConfigError, HomeError, Source, describe_package_roots_miss_in, ensure_journal_dir_with_label,
    read_config_journal, resolve_installation_root_from_executable_dir, resolve_journal_path,
};
#[cfg(feature = "journal-mcp-endpoint")]
use solstone_core_journal_config::{McpEndpointCapability, mcp_endpoint_capability};
use solstone_core_journal_config::{materialized_defaults, read_journal_config};
use solstone_core_journal_config_write::{
    CommitConfigError, ConfigExpectation, LockError, LockOptions, commit_journal_config,
};
mod talent_preview;
mod warm;
const EXIT_USAGE: u8 = 64;
const EXIT_DATAERR: u8 = 65;
#[allow(dead_code)]
const EXIT_NOINPUT: u8 = 66;
#[allow(dead_code)]
const EXIT_NOHOST: u8 = 68;
const EXIT_UNAVAILABLE: u8 = 69;
const EXIT_INTERNAL_FAILURE: u8 = 70;
const EXIT_TEMPFAIL: u8 = 75;
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
const ZERO_EDGE_HINT: &str = "No new connections were added in this rescan. Run solstone journal indexer --rebuild-edges to bring any still waiting up to date now.";
struct JournalPathLine {
    label: &'static str,
    path: PathBuf,
}

enum JournalPathError {
    Config(ConfigError),
    Home(HomeError),
    Create(solstone_core_journal::EnsureJournalDirError),
}

/// Use the grammar of this invocation when rendering diagnostics before dispatch.
mod host_copy;
use host_copy::{host_display, host_locale, host_text};

// The portable catalog accepts signed 32-bit selectors. Keep the original count
// for display and preserve plural-rule residues for larger values.
fn host_count_quantity(value: u64) -> i64 {
    if value <= i32::MAX as u64 {
        value as i64
    } else {
        (1_000_000 + value % 1_000_000) as i64
    }
}

fn install_logger() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
}

#[cfg(any(unix, windows))]
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
#[cfg(any(unix, windows))]
fn run_hosted_service<F>(
    journal: &Path,
    kind: HostedServiceKind,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
    run: F,
) -> ExitCode
where
    F: FnOnce(Option<Arc<HostedServiceParentRuntime>>) -> ExitCode,
{
    match admit_hosted_service_parent(
        journal,
        kind,
        #[cfg(windows)]
        admitted,
    ) {
        Ok(parent) => run(parent.map(Arc::new)),
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.hosted.service.hosted.service.admission.refused",
                    [("service", &(kind_name(kind))), ("error", &(error))]
                )
            );
            ExitCode::from(EXIT_HOSTED_SERVICE_ADMISSION_REFUSED)
        }
    }
}

#[cfg(any(unix, windows))]
const fn kind_name(kind: HostedServiceKind) -> &'static str {
    match kind {
        HostedServiceKind::Convey => "convey",
        HostedServiceKind::Sense => "sense",
        HostedServiceKind::Cortex => "cortex",
        HostedServiceKind::Spl => "spl",
        HostedServiceKind::Mcp => "mcp",
        HostedServiceKind::McpLocalDoor => "mcp_local_door",
    }
}

#[cfg(any(unix, windows))]
fn run_supervisor(
    options: solstone_core_cli::SupervisorOptions,
    #[cfg(windows)] installed_task: Option<
        &solstone_core_system::process::AdmittedInstalledTaskLaunch,
    >,
) -> ExitCode {
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
    #[cfg(unix)]
    if solstone_core_system::storage_privacy::drive_cannot_run_journal(&journal) {
        eprintln!(
            "{}",
            solstone_core_window_catalog::text(host_locale(), "journal.setup.drive.unsupported")
        );
        return ExitCode::from(EXIT_UNAVAILABLE);
    }
    // One journal runs on a computer at a time. Another account's holds the
    // claim, and the Journal app reads the same fact to say so.
    #[cfg(windows)]
    if !options.no_convey && !solstone_core_local_endpoint::claim_journal_on_this_computer() {
        eprintln!(
            "{}",
            host_text(
                "journal.cli.host.supervisor.your.journal.can.t.start.because.another.account.on.this"
            )
        );
        return ExitCode::from(EXIT_UNAVAILABLE);
    }
    solstone_core_system::storage_privacy::apply_private_root(&journal);
    // This journal already running keeps its own plain refusal, further on.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if !options.no_convey
        && browser_port_in_use(options.port)
        && !solstone_core_local_endpoint::journal_is_up(&journal)
    {
        eprintln!(
            "{}",
            host_display(
                "journal.cli.host.supervisor.your.journal.can.t.start.because.port.on.this.computer",
                [("port", &(options.port))]
            )
        );
        return ExitCode::from(EXIT_UNAVAILABLE);
    }
    #[cfg(unix)]
    let _service_capture = match service_capture::start_if_managed(&journal, options.hosted_parent)
    {
        Ok(capture) => capture,
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.supervisor.supervisor.service.capture.unavailable",
                    [("error", &(error))]
                )
            );
            return ExitCode::from(EXIT_CANTCREAT);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.supervisor.supervisor.runtime.unavailable",
                    [("error", &(error))]
                )
            );
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    render_supervisor_host_outcome(runtime.block_on(supervisor::run_hosted_in(
        host_locale(),
        &journal,
        options,
        parent,
        #[cfg(windows)]
        installed_task,
    )))
}

/// Whether something already holds the browser port. A bind conflict proves
/// the port is busy, not who holds it, so nothing here names or probes it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn browser_port_in_use(port: u16) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};
    port != 0
        && [
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)),
            TcpListener::bind((Ipv6Addr::LOCALHOST, port)),
        ]
        .iter()
        .any(|bound| {
            bound
                .as_ref()
                .is_err_and(|error| error.kind() == std::io::ErrorKind::AddrInUse)
        })
}

#[cfg(any(unix, windows))]
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
            eprintln!("{}", refusal.message_for(host_locale()));
            ExitCode::from(EXIT_TEMPFAIL)
        }
        #[cfg(unix)]
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::AdmissionWaitTerminal,
        } => {
            eprintln!(
                "{}",
                host_text("journal.cli.host.supervisor.admission.wait.terminal")
            );
            ExitCode::from(EXIT_TEMPFAIL)
        }
        #[cfg(unix)]
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::AdmissionWaitUnverifiable,
        } => {
            eprintln!(
                "{}",
                host_text("journal.cli.host.supervisor.admission.wait.unverifiable")
            );
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::LifecycleRecovery(message),
        } => {
            eprintln!("{message}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::AlreadyRunning,
        } => {
            eprint!(
                "{}",
                host_text("journal.cli.host.supervisor.already.running")
            );
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::Refused {
            reason: supervisor::SupervisorBootRefusal::SpeakersAnalyzeGeneration(message),
        } => {
            eprintln!("{message}");
            ExitCode::from(EXIT_SPEAKERS_ANALYZE_GENERATION_REFUSED)
        }
        supervisor::SupervisorHostOutcome::Refused { reason } => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.supervisor.host.outcome.supervisor.failed.to.boot",
                    [("reason", &(format!("{:?}", reason)))]
                )
            );
            ExitCode::from(EXIT_TEMPFAIL)
        }
        supervisor::SupervisorHostOutcome::LifecycleShutdownFailed {
            cause,
            readiness,
            self_heartbeat,
            identity,
        } => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.supervisor.host.outcome.supervisor.lifecycle.shutdown.failed.cause.readiness.self.heartbeat.identity",
                    [
                        ("cause", &(format!("{:?}", cause))),
                        ("readiness", &(format!("{:?}", readiness))),
                        ("self_heartbeat", &(format!("{:?}", self_heartbeat))),
                        ("identity", &(format!("{:?}", identity)))
                    ]
                )
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

#[cfg(any(unix, windows))]
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
        // A session end is a requested stop, so it exits clean. This matters
        // beyond tidiness on Windows: the installed forwarder restarts a
        // supervisor that exits non-zero, and a logoff is not a crash.
        supervisor::ShutdownCause::HostSessionEnd | supervisor::ShutdownCause::Signal(_) => 0,
    }
}

#[cfg(not(unix))]
fn unavailable() -> ExitCode {
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(not(any(unix, windows)))]
fn run_supervisor(_options: solstone_core_cli::SupervisorOptions) -> ExitCode {
    unavailable()
}

#[cfg(unix)]
fn run_journal_route_inspect() -> ExitCode {
    journal_route::inspect()
}

#[cfg(not(unix))]
fn run_journal_route_inspect() -> ExitCode {
    unavailable()
}

#[cfg(unix)]
fn run_journal_route_repair(lock_owner: &str) -> ExitCode {
    journal_route::repair(lock_owner)
}

#[cfg(not(unix))]
fn run_journal_route_repair(_lock_owner: &str) -> ExitCode {
    unavailable()
}

#[cfg(any(unix, windows))]
fn run_brain_owner(command: JournalBrainOwnerCommand) -> ExitCode {
    brain_owner::run(command)
}

#[cfg(not(any(unix, windows)))]
fn run_brain_owner(_command: JournalBrainOwnerCommand) -> ExitCode {
    unavailable()
}

#[cfg(any(unix, windows))]
fn run_heartbeat(force: bool) -> ExitCode {
    match resolve_process_journal_path() {
        Ok(resolved) => heartbeat::run(&resolved.path, force),
        Err(error) => print_journal_error(error),
    }
}

#[cfg(not(any(unix, windows)))]
fn run_heartbeat(_force: bool) -> ExitCode {
    unavailable()
}

#[cfg(unix)]
fn run_service(outcome: ServiceParseOutcome) -> ExitCode {
    match outcome {
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
    }
}

#[cfg(windows)]
fn run_service(outcome: ServiceParseOutcome) -> ExitCode {
    match outcome {
        ServiceParseOutcome::Dispatch(ServiceAction::Logs { follow }) => {
            service_logs::run(solstone_core_cli::ServiceLogsArgs { follow })
        }
        ServiceParseOutcome::Dispatch(action) => service_windows::run(action),
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
    }
}

#[cfg(not(any(unix, windows)))]
fn run_service(outcome: ServiceParseOutcome) -> ExitCode {
    match outcome {
        ServiceParseOutcome::Dispatch(ServiceAction::Logs { follow }) => {
            service_logs::run(solstone_core_cli::ServiceLogsArgs { follow })
        }
        ServiceParseOutcome::Dispatch(_) => unavailable(),
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
    }
}

fn main() -> ExitCode {
    #[cfg(unix)]
    solstone_core_system::storage_privacy::set_private_umask();
    let args: Vec<_> = env::args_os().skip(1).collect();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(code) = solstone_core_backup::owned_dns::dispatch(&args) {
        return ExitCode::from(code);
    }
    #[cfg(target_os = "macos")]
    if let Some(code) = solstone_core_backup::owned_creator::dispatch(&args) {
        return ExitCode::from(code);
    }
    #[cfg(windows)]
    let installed_task = match service_windows::admit_task_action(&args) {
        Ok(action) => action,
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.main.windows.service.admission.failed",
                    [("error", &(error))]
                )
            );
            return ExitCode::from(EXIT_HOSTED_SERVICE_ADMISSION_REFUSED);
        }
    };
    #[cfg(windows)]
    if let Some(action) = installed_task {
        let root = match solstone_core_system::process::receive_windows_installed_task_launch(
            &action.launch,
        ) {
            Ok(root) => root,
            Err(error) => {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.main.windows.service.admission.failed",
                        [("error", &(error))]
                    )
                );
                return ExitCode::from(EXIT_HOSTED_SERVICE_ADMISSION_REFUSED);
            }
        };
        // The actual root admission remains owned until capture and runtime return.
        return service_capture_windows::run_installed(&action.journal, || {
            install_logger();
            if let Err(error) = solstone_core_system::process::watch_windows_session_end() {
                log::warn!("supervisor: session-end window unavailable: {error}");
            }
            match solstone_core_cli::evaluate_args(&action.arguments) {
                Ok(Command::Supervisor(options)) => run_supervisor(options, Some(&root)),
                _ => ExitCode::from(EXIT_HOSTED_SERVICE_ADMISSION_REFUSED),
            }
        });
    }
    #[cfg(windows)]
    let admitted = match solstone_core_system::process::receive_windows_launch() {
        Ok(admitted) => admitted,
        Err(error) => {
            eprintln!(
                "{}",
                host_display("journal.cli.host.process.admission", [("error", &(error))])
            );
            return ExitCode::from(EXIT_HOSTED_SERVICE_ADMISSION_REFUSED);
        }
    };
    install_logger();
    #[cfg(feature = "social-memory-dev")]
    if args.first().is_some_and(|command| command == "__social") {
        return run_social_dev(&args[1..]);
    }
    let locale = host_locale();
    match solstone_core_cli::evaluate_args_for(locale, &args) {
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
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.config.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ConfigUsage) => render_usage_error(
            locale,
            "journal.cli.host.config.usage",
            "solstone journal config",
        ),
        Ok(Command::CheckUsage) => render_usage_error(
            locale,
            "journal.cli.host.check.usage",
            "solstone journal check",
        ),
        Ok(Command::CheckHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.check.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Doctor(args)) => run_doctor(args),
        Ok(Command::DoctorHelp) => {
            println!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.doctor.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::DoctorUsage(error)) => {
            eprintln!("{}", error.message_for(locale));
            ExitCode::from(2)
        }
        Ok(Command::Setup(args)) => run_setup(args),
        Ok(Command::JournalRouteInspect) => run_journal_route_inspect(),
        Ok(Command::JournalRouteRepair { lock_owner }) => run_journal_route_repair(&lock_owner),
        Ok(Command::SetupHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.setup.usage", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::SetupUsage(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.setup.usage", &[])
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.setup.error",
                    [("detail", &error.0)]
                )
            );
            ExitCode::from(2)
        }
        Ok(Command::SetupCopy(args)) => {
            let is_help = args.len() == 1 && (args[0] == "--help" || args[0] == "-h");
            if is_help {
                println!("{}", host_text("journal.cli.journal.help.setup.copy"));
                ExitCode::SUCCESS
            } else if args.len() == 1 && args[0] == "--json" {
                let facts = solstone_core_thinking_copy::current_setup_facts();
                let copy = solstone_core_thinking_copy::localized_setup_copy(
                    &facts,
                    solstone_core_window_catalog::cli_locale(),
                );
                let output = format!(
                    "{}\n",
                    serde_json::to_string_pretty(&copy).expect("setup copy serializes")
                );
                print!("{output}");
                ExitCode::SUCCESS
            } else {
                eprintln!("{}", host_text("journal.cli.journal.help.setup.copy"));
                ExitCode::from(2)
            }
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
        Ok(Command::Brain(command)) => run_brain(command),
        Ok(Command::JournalBrainOwner(command)) => match command {
            JournalBrainOwnerCommand::Help => {
                println!(
                    "{}",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.brain.help.root", &[])
                );
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::StatusHelp => {
                println!(
                    "{}",
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.brain.help.status",
                        &[]
                    )
                );
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::RefreshHelp => {
                println!(
                    "{}",
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.brain.help.refresh",
                        &[]
                    )
                );
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::RenewPrerequisitesHelp => {
                println!(
                    "{}",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.brain.help.renew", &[])
                );
                ExitCode::SUCCESS
            }
            JournalBrainOwnerCommand::Bare => {
                println!(
                    "{}",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.brain.help.root", &[])
                );
                ExitCode::from(2)
            }
            JournalBrainOwnerCommand::Usage => {
                eprintln!(
                    "{}\n{}",
                    solstone_core_cli_copy::render_for(locale, "journal.cli.brain.usage", &[]),
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.brain.arguments.invalid",
                        &[]
                    )
                );
                ExitCode::from(2)
            }
            command => run_brain_owner(command),
        },
        Ok(Command::Transcribe(options)) => run_transcribe(
            options,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::Think(args)) => run_storage_ops_verb("think", args, |arguments, journal| {
            // Only the whole-day lifecycle (the sole caller of `sense_batch`)
            // needs a speakers-analyze generation; narrower modes (`--segment`,
            // `--flush`, `--weekly`, `--cadence`, `--activity`, `--updated`,
            // `--segments`, `--dry-run`) never reach it and must not be forced
            // to contend for a lease they don't need.
            if !solstone_core_think_cli::requires_daily_lifecycle(arguments) {
                let run = solstone_core_think_cli::run_cli(
                    arguments,
                    journal,
                    &solstone_core_system::process::ChildLaunchContext::default(),
                );
                return (run.stdout, run.stderr, run.exit_code);
            }
            let locale = solstone_core_think_cli::terminal_locale()
                .unwrap_or(solstone_core_cli_copy::ENGLISH);
            let generation = match solstone_core_transcribe::enter_speakers_analyze_generation_in(
                locale,
                journal,
                solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Think,
                #[cfg(windows)]
                admitted.as_ref(),
            ) {
                Ok(generation) => generation,
                Err(error) => {
                    return (
                        String::new(),
                        solstone_core_transcribe::speakers_installation_detail_in(&error, locale)
                            .unwrap_or_default(),
                        error.exit_code(),
                    );
                }
            };
            let sense_child_environment = generation.child_launch_context();
            let run =
                solstone_core_think_cli::run_cli(arguments, journal, &sense_child_environment);
            drop(generation);
            (run.stdout, run.stderr, run.exit_code)
        }),
        Ok(Command::ThinkingHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.thinking.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ThinkingUsage) => render_usage_error(
            locale,
            "journal.cli.host.thinking.usage",
            "solstone journal thinking",
        ),
        Ok(Command::Thinking(ThinkingCommand::SetLaneHelp)) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.thinking.set.lane.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::SetLaneUsage)) => render_usage_error(
            locale,
            "journal.cli.host.thinking.set.lane.usage",
            "solstone journal thinking set-lane",
        ),
        Ok(Command::Thinking(ThinkingCommand::SetLane(options))) => thinking::run(options),
        Ok(Command::Thinking(ThinkingCommand::ChatGptHelp)) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.thinking.chatgpt.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGptUsage)) => render_usage_error(
            locale,
            "journal.cli.host.thinking.chatgpt.usage",
            "solstone journal thinking chatgpt",
        ),
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::SignInHelp))) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.thinking.chatgpt.sign.in.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::SignInUsage))) => {
            render_usage_error(
                locale,
                "journal.cli.host.thinking.chatgpt.sign.in.usage",
                "solstone journal thinking chatgpt sign-in",
            )
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::SignIn(options)))) => {
            thinking::run_chatgpt_sign_in(options)
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::SignOutHelp))) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.thinking.chatgpt.sign.out.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::SignOutUsage))) => {
            render_usage_error(
                locale,
                "journal.cli.host.thinking.chatgpt.sign.out.usage",
                "solstone journal thinking chatgpt sign-out",
            )
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::SignOut(options)))) => {
            thinking::run_chatgpt_sign_out(options)
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::StatusHelp))) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.thinking.chatgpt.status.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::StatusUsage))) => {
            render_usage_error(
                locale,
                "journal.cli.host.thinking.chatgpt.status.usage",
                "solstone journal thinking chatgpt status",
            )
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::Status(options)))) => {
            thinking::run_chatgpt_status(options)
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::ModelsHelp))) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.thinking.chatgpt.models.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::ModelsUsage))) => {
            render_usage_error(
                locale,
                "journal.cli.host.thinking.chatgpt.models.usage",
                "solstone journal thinking chatgpt models",
            )
        }
        Ok(Command::Thinking(ThinkingCommand::ChatGpt(ChatGptCommand::Models(options)))) => {
            thinking::run_chatgpt_models(options)
        }
        Ok(Command::Streams(args)) => run_streams(args),
        Ok(Command::Importer(args)) => run_importer(args),
        Ok(Command::Segment(args)) => run_segment(args),
        Ok(Command::Backup(args)) => run_backup(args),
        Ok(Command::Maintenance(args)) => run_maintenance(
            args,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::TalentWorker(args)) => run_talent_worker(args),
        Ok(Command::PossibleNamesPacket(segment)) => run_possible_names_packet(&segment),
        Ok(Command::ParentLossCoordinator(args)) => run_parent_loss_coordinator(args),
        Ok(Command::Reprocess(args)) => run_reprocess(args),
        Ok(Command::JournalStats(args)) => run_journal_stats(args),
        Ok(Command::Talent(args)) => run_talent(args),
        Ok(Command::Share(args)) => run_share(args),
        Ok(Command::Backfill(args)) => run_backfill(args),
        Ok(Command::BackfillFacetIds { commit }) => run_backfill_facet_ids(commit),
        Ok(Command::BackfillFacetIdsHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.backfill.facet.ids.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::BackfillFacetIdsUsage) => render_usage_error(
            locale,
            "journal.cli.host.backfill.facet.ids.usage",
            "solstone journal backfill-facet-ids",
        ),
        Ok(Command::FacetCandidates) => run_facet_candidates(),
        Ok(Command::InstallModels(options)) => install_models::run(options),
        Ok(Command::InstallModelsUsage) => render_usage_error(
            locale,
            "journal.cli.host.install.models.usage",
            "solstone journal install-models",
        ),
        Ok(Command::InstallModelsHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.install.models.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::InstallProvider(options)) => install_provider::run(options),
        Ok(Command::InstallProviderUsage) => render_usage_error(
            locale,
            "journal.cli.host.install.provider.usage",
            "solstone journal install-provider",
        ),
        Ok(Command::InstallProviderHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.install.provider.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Convey(options)) => run_convey(
            options,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::ConveyHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.convey.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ConveyUsage(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.convey.usage", &[])
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::Schedule(options)) => run_schedule(options),
        Ok(Command::ScheduleHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.schedule.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ScheduleUsage(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.schedule.usage", &[])
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::Grab(command)) => run_grab(command),
        Ok(Command::Spl(command)) => run_spl_process(
            command,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::SplUsage(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.spl.usage", &[])
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::SplHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.spl.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Mcp(command)) => run_mcp_process(
            command,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::McpUsage(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.mcp.usage", &[])
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::McpHelp(help)) => {
            print!("{help}");
            ExitCode::SUCCESS
        }
        Ok(Command::Sense(options)) => run_sense(
            options,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::SenseUsage) => render_usage_error(
            locale,
            "journal.cli.host.sense.usage",
            "solstone journal sense",
        ),
        Ok(Command::SenseHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.sense.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Cortex(options)) => run_cortex_service(
            options,
            #[cfg(windows)]
            admitted.as_ref(),
        ),
        Ok(Command::CortexUsage(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.cortex.usage", &[])
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::CortexHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.cortex.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Supervisor(options)) => run_supervisor(
            options,
            #[cfg(windows)]
            None,
        ),
        Ok(Command::SupervisorUsage) => render_usage_error(
            locale,
            "journal.cli.host.supervisor.usage",
            "solstone journal supervisor",
        ),
        Ok(Command::SupervisorInvalid(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.supervisor.usage",
                    &[]
                )
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::SupervisorHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.supervisor.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::StartUsage) => render_usage_error(
            locale,
            "journal.cli.host.start.usage",
            "solstone journal start",
        ),
        Ok(Command::StartInvalid(error)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.start.usage", &[])
            );
            eprintln!("{}", error.0);
            ExitCode::from(2)
        }
        Ok(Command::StartHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.start.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::SupervisorLifecycleRedirect(verb)) => {
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.supervisor.lifecycle.redirect",
                    [
                        ("supervisor", &"solstone journal supervisor"),
                        ("service_verb", &"solstone journal service <verb>"),
                        ("requested", &format!("solstone journal service {verb}")),
                    ]
                )
            );
            ExitCode::from(2)
        }
        Ok(Command::Health { verbose, debug }) => health::run(verbose, debug),
        Ok(Command::HealthUsage) => render_usage_error(
            locale,
            "journal.cli.host.health.usage",
            "solstone journal health",
        ),
        Ok(Command::HealthHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.health.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Top { verbose, debug }) => match solstone_core_top::run(verbose, debug) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!(
                    "{}",
                    host_display("journal.cli.host.top.failure", [("error", &(error))])
                );
                ExitCode::from(EXIT_UNAVAILABLE)
            }
        },
        Ok(Command::TopUsage) => {
            render_usage_error(locale, "journal.cli.host.top.usage", "solstone journal top")
        }
        Ok(Command::TopHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.top.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::HealthLogs(args)) => health_logs::run(args),
        Ok(Command::HealthLogsUsage(args)) => health_logs::usage(args),
        Ok(Command::HealthLogsHelp(args)) => health_logs::help(args),
        Ok(Command::Heartbeat { force }) => run_heartbeat(force),
        Ok(Command::HeartbeatUsage) => render_usage_error(
            locale,
            "journal.cli.host.heartbeat.usage",
            "solstone journal heartbeat",
        ),
        Ok(Command::HeartbeatHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.heartbeat.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::Service(outcome)) => run_service(outcome),
        Ok(Command::Open { path, print_url }) => open::run(path, print_url),
        Ok(Command::OpenHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.open.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::OpenUsage) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.open.usage", &[])
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.arguments.invalid",
                    [("command", &("solstone journal open"))]
                )
            );
            ExitCode::from(2)
        }
        Ok(Command::Navigate { path }) => navigate::run(path),
        Ok(Command::NavigateHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.navigate.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::NavigateUsage) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.navigate.usage", &[])
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.arguments.invalid",
                    [("command", &("solstone journal navigate"))]
                )
            );
            ExitCode::from(2)
        }
        Ok(Command::NavigateFacetRetired(option)) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.navigate.usage", &[])
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.navigate.error.is.no.longer.supported.put.facet.selection.in.the.destination.url.for",
                    [("option", &(option))]
                )
            );
            ExitCode::from(2)
        }
        Ok(Command::Contract(ContractCommand::Build { check, root })) => {
            contract::run_build(check, root)
        }
        Ok(Command::Contract(ContractCommand::Check { journals, root })) => {
            contract::run_check(journals, root)
        }
        Ok(Command::ContractHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.contract.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ContractUsage) => render_usage_error(
            locale,
            "journal.cli.host.contract.usage",
            "solstone journal contract",
        ),
        Ok(Command::ContractBuildHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.contract.build.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ContractBuildUsage) => render_usage_error(
            locale,
            "journal.cli.host.contract.build.usage",
            "solstone journal contract build",
        ),
        Ok(Command::ContractCheckHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.contract.check.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::ContractCheckUsage) => render_usage_error(
            locale,
            "journal.cli.host.contract.check.usage",
            "solstone journal contract check",
        ),
        Ok(Command::Settings(command)) => settings::run(command),
        Ok(Command::SettingsHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.settings.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::SettingsConveyHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.settings.convey.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::SettingsStatusHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.settings.status.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::SettingsParseError(error)) => settings_parse_error(locale, error),
        Ok(Command::TranscribeHelp) => {
            println!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.transcribe.help", &[])
            );
            ExitCode::SUCCESS
        }
        Ok(Command::FacetCandidatesHelp) => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.facet.candidates.help",
                    &[]
                )
            );
            ExitCode::SUCCESS
        }
        Ok(Command::FacetCandidatesUsage) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.facet.candidates.usage",
                    &[]
                )
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.arguments.invalid",
                    [("command", &("solstone journal facet-candidates"))]
                )
            );
            ExitCode::from(2)
        }
        Err(_) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.usage.commands",
                    [("commands", &solstone_core_cli::USAGE_COMMANDS)]
                )
            );
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn render_usage_error(locale: &str, usage_id: &str, command: &str) -> ExitCode {
    eprint!(
        "{}",
        solstone_core_cli_copy::render_for(locale, usage_id, &[])
    );
    eprintln!(
        "{}",
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.host.arguments.invalid",
            [("command", &(command))]
        )
    );
    ExitCode::from(2)
}

fn settings_parse_error(locale: &str, error: SettingsParseError) -> ExitCode {
    match error {
        SettingsParseError::InvalidSection(value) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.settings.usage", &[])
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.settings.error.argument.section.invalid.choice.choose.from.convey",
                    [("value", &(value))]
                )
            );
        }
        SettingsParseError::InvalidConveyCommand(value) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.host.settings.convey.usage",
                    &[]
                )
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.settings.convey.error.argument.convey.command.invalid.choice.choose.from.status",
                    [("value", &(value))]
                )
            );
        }
        SettingsParseError::UnrecognizedArgument(value) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.settings.usage", &[])
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.host.settings.error.unrecognized.arguments",
                    [("value", &(value))]
                )
            );
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
                "{}",
                host_display(
                    "journal.cli.host.malformed.jsonl.warning.solstone.observe.utils.invalid.json.at.line.in",
                    [
                        ("line", &(line)),
                        ("path", &(path.display())),
                        ("error", &(error))
                    ]
                )
            );
        }
    }

    fn read_error(&mut self, path: &Path, error: &io::Error) {
        eprintln!(
            "{}",
            host_display(
                "journal.cli.host.read.error.error.solstone.observe.utils.error.reading",
                [("path", &(path.display())), ("error", &(error))]
            )
        );
    }
}

fn run_grab(command: GrabCommand) -> ExitCode {
    let locale = host_locale();
    match command {
        GrabCommand::Help => {
            print!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.grab.help", &[])
            );
            ExitCode::SUCCESS
        }
        GrabCommand::ParseError(message) => {
            eprint!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.host.grab.usage", &[])
            );
            // argparse prefixes the verb; the bare `error:` named no command.
            eprintln!("{message}");
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

fn run_transcribe(
    options: TranscribeOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let terminal = solstone_core_transcribe::terminal_locale();
    let locale = terminal.unwrap_or(solstone_core_cli_copy::ENGLISH);
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error_in(locale, error);
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
    match solstone_core_transcribe::run_cli_in(
        terminal,
        options.arguments,
        &journal.path,
        &mut on_day,
        #[cfg(windows)]
        admitted,
    ) {
        Ok(result) => complete_transcribe(&journal.path, result),
        Err(CliRunError::Cli(CliError::Usage { message })) => {
            eprint!(
                "{}",
                solstone_core_transcribe::usage_error(locale, &message)
            );
            ExitCode::from(2)
        }
        Err(error) => {
            if let Some(locale) = terminal {
                eprintln!("{}", error.render(locale));
            } else {
                eprintln!("{error}");
            }
            ExitCode::from(error.exit_code() as u8)
        }
    }
}

fn complete_transcribe(journal: &Path, result: solstone_core_transcribe::CliRun) -> ExitCode {
    feed_transcribed_files(journal, &result.files);
    if let Some(summary) = &result.summary {
        println!("{summary}");
    }
    if let Some(stderr) = &result.stderr {
        eprint!("{stderr}");
    }
    ExitCode::from(result.exit_code as u8)
}

fn feed_transcribed_files(journal: &Path, files: &[solstone_core_transcribe::TranscribeFile]) {
    for file in files {
        if file.outcome != solstone_core_transcribe::TranscribeOutcome::Transcribed {
            continue;
        }
        if let Err(error) =
            solstone_core_speaker_resolve::transcribed_clusters::feed_transcribed_clusters(
                journal,
                &file.transcript_path,
                &file.embeddings_path,
                &|| Ok(()),
            )
        {
            let locale = solstone_core_transcribe::terminal_locale()
                .unwrap_or(solstone_core_cli_copy::ENGLISH);
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.transcribe.feed.failure",
                    [
                        ("path", &(file.transcript_path.display())),
                        ("detail", &(error.render(locale)))
                    ]
                )
            );
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
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    host_locale(),
                    "journal.cli.reprocess.native.arguments",
                    [("verb", &verb)]
                )
            );
            Err(ExitCode::from(EXIT_TEMPFAIL))
        }
    }
}

fn run_storage_ops_verb(
    verb: &str,
    args: Vec<OsString>,
    call: impl FnOnce(&[String], &Path) -> (String, String, i32),
) -> ExitCode {
    let (arguments, journal_path) = match storage_ops_arguments(verb, args) {
        Ok(input) => input,
        Err(code) => return code,
    };
    let (stdout, stderr, exit_code) = call(&arguments, &journal_path);
    print!("{stdout}");
    eprint!("{stderr}");
    ExitCode::from(exit_code as u8)
}

fn storage_ops_arguments(
    verb: &str,
    args: Vec<OsString>,
) -> Result<(Vec<String>, PathBuf), ExitCode> {
    let arguments = require_utf8_argv(verb, &args)?;
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
                return Err(ExitCode::from(EXIT_TEMPFAIL));
            }
        }
    };
    Ok((arguments, journal_path))
}

enum OwnedBackupCommand {
    Backup(solstone_core_backup_cli::CliOutcome),
    Maintenance(solstone_core_maintenance::CliOutcome),
}

fn run_owned_backup_command(
    verb: &str,
    args: Vec<OsString>,
    call: impl FnOnce(&[String], &Path) -> OwnedBackupCommand,
) -> ExitCode {
    let (arguments, journal) = match storage_ops_arguments(verb, args) {
        Ok(input) => input,
        Err(code) => return code,
    };
    // Ctrl-C or SIGTERM stops running archive children instead of orphaning them.
    #[cfg(unix)]
    solstone_core_backup::bounded_child::stop_archive_work_on_signals();
    let owned = call(&arguments, &journal);
    let (stdout, stderr, code) = match &owned {
        OwnedBackupCommand::Backup(run) => run.output(),
        OwnedBackupCommand::Maintenance(run) => run.output(),
    };
    print!("{stdout}");
    eprint!("{stderr}");
    // The actual whole outcome remains alive through both print operations.
    ExitCode::from(code as u8)
}

fn run_streams(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("streams", args, |arguments, journal| {
        let run = solstone_core_streams_cli::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_importer(args: Vec<OsString>) -> ExitCode {
    let arguments = match require_utf8_argv("importer", &args) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    let (stdout, stderr, code) = match importer_desktop::run(&arguments, || {
        resolve_process_journal_path().map(|journal| journal.path)
    }) {
        Ok(output) => output,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    print!("{stdout}");
    eprint!("{stderr}");
    ExitCode::from(code as u8)
}

fn run_segment(args: Vec<OsString>) -> ExitCode {
    run_storage_ops_verb("segment", args, |arguments, journal| {
        let run = solstone_core_segment_cli::run_cli(arguments, journal);
        (run.stdout, run.stderr, run.exit_code)
    })
}

fn run_backup(args: Vec<OsString>) -> ExitCode {
    run_owned_backup_command("backup", args, |arguments, journal| {
        OwnedBackupCommand::Backup(solstone_core_backup_cli::run_cli(arguments, journal))
    })
}

fn run_maintenance(
    args: Vec<OsString>,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    run_owned_backup_command("maintenance", args, |arguments, journal| {
        #[cfg(windows)]
        let _speakers_generation =
            if solstone_core_maintenance::needs_inference_generation(arguments, journal) {
                match solstone_core_transcribe::enter_speakers_analyze_generation(
                    journal,
                    solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Maintenance,
                    admitted,
                ) {
                    Ok(generation) => Some(generation),
                    Err(error) => {
                        return OwnedBackupCommand::Maintenance(
                            solstone_core_maintenance::CliOutcome::Rendered(
                                solstone_core_maintenance::CliRun {
                                    stdout: String::new(),
                                    stderr: error
                                        .message()
                                        .map(|message| format!("{message}\n"))
                                        .unwrap_or_default(),
                                    exit_code: error.exit_code(),
                                },
                            ),
                        );
                    }
                }
            } else {
                None
            };
        #[cfg(windows)]
        if let Some(generation) = _speakers_generation {
            let run = solstone_core_maintenance::run_cli_with_discovery_generation(
                arguments,
                journal,
                &generation.child_launch_context(),
            );
            return OwnedBackupCommand::Maintenance(
                solstone_core_maintenance::CliOutcome::Rendered(run),
            );
        }
        OwnedBackupCommand::Maintenance(solstone_core_maintenance::run_cli_in(
            solstone_core_cli_copy::terminal_locale().map(|_| host_locale()),
            arguments,
            journal,
        ))
    })
}

#[cfg(any(unix, windows))]
fn run_talent_worker(args: Vec<OsString>) -> ExitCode {
    let arguments = match require_utf8_argv("talent worker", &args) {
        Ok(arguments) => arguments,
        Err(code) => return code,
    };
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => return print_journal_error(error),
    };
    #[cfg(unix)]
    if let Err(error) = acknowledge_hosted_child_admission(&journal) {
        eprintln!("talent worker parent-loss admission failed: {error}");
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    solstone_core_talent_runtime::run_worker(&arguments, &journal)
}

/// Evaluation only: the possible-names request for one segment, as JSON.
#[cfg(any(unix, windows))]
fn run_possible_names_packet(segment: &std::path::Path) -> ExitCode {
    let report =
        solstone_core_talent_runtime::runtime_paths_from_current_executable().and_then(|paths| {
            solstone_core_talent_runtime::possible_names::inspect_packet(segment, &paths)
        });
    match report {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("report serializes")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("possible names packet: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn run_possible_names_packet(_segment: &std::path::Path) -> ExitCode {
    unavailable()
}

#[cfg(not(any(unix, windows)))]
fn run_talent_worker(_args: Vec<OsString>) -> ExitCode {
    unavailable()
}

#[cfg(unix)]
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
            // stderr is not kept; the supervisor logs what arrives here.
            let mut channel = std::io::stdout();
            let _ = writeln!(channel, "bootstrap failed: {error}");
            let _ = channel.flush();
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    // The supervisor closes its end of the refusal channel once this process
    // is ready, so a later write to it would fail. Nothing writes to stdout
    // after this point, and /dev/null keeps it that way.
    if let Ok(null) = std::fs::OpenOptions::new().write(true).open("/dev/null") {
        let _ = nix::unistd::dup2_stdout(null);
    }
    match coordinator.run() {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("parent-loss coordinator: lifecycle failed: {error}");
            ExitCode::from(EXIT_TEMPFAIL)
        }
    }
}

#[cfg(not(unix))]
fn run_parent_loss_coordinator(_args: Vec<OsString>) -> ExitCode {
    unavailable()
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
                "{}",
                solstone_core_cli_copy::strings(
                    "journal.cli.stats.roots",
                    [(
                        "error",
                        &describe_package_roots_miss_in(
                            executable_dir.as_deref().unwrap_or(Path::new("")),
                            solstone_core_cli_copy::cli_locale()
                        )
                    )]
                )
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
        jiff::Timestamp::now(),
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
    let locale = solstone_core_talent_cli::output_locale(args.iter().any(|arg| arg == "--json"));
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
                eprint_journal_path_error_in(locale, error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let executable = match env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                eprintln!(
                    "{}",
                    solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.talent.startup.executable",
                        [("error", &(error))]
                    )
                );
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let Some(executable_dir) = executable.parent() else {
            eprintln!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.talent.startup.package",
                    &[]
                )
            );
            return ExitCode::from(EXIT_TEMPFAIL);
        };
        let Some(root) = resolve_installation_root_from_executable_dir(executable_dir) else {
            eprintln!(
                "{}",
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.talent.startup.package",
                    &[]
                )
            );
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

/// `journal share`: read-only matching and packaging. Talents run in process
/// through the installed talent tree and the sibling generate client.
/// `__social`: social memory's sign-in and share, in development builds only.
#[cfg(feature = "social-memory-dev")]
fn run_social_dev(args: &[OsString]) -> ExitCode {
    use solstone_core_cli_copy::{ENGLISH, terminal_locale};
    solstone_core_cli_copy::begin_terminal_output();
    let locale = terminal_locale().unwrap_or(ENGLISH);
    let journal = if solstone_core_social_memory::cli::is_help(args) {
        PathBuf::new()
    } else {
        match resolve_process_journal_path() {
            Ok(journal) => journal.path,
            Err(error) => {
                eprint_journal_path_error_in(locale, error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        }
    };
    let code = solstone_core_social_memory::cli::run(args, &journal, locale);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn run_share(args: Vec<OsString>) -> ExitCode {
    use solstone_core_cli_copy::{ENGLISH, MessageDetail, terminal_locale};
    use solstone_core_generate::SiblingError;
    solstone_core_cli_copy::begin_terminal_output();
    let locale = terminal_locale().unwrap_or(ENGLISH);
    let run = if !solstone_core_linked_share::reads_journal(&args) {
        solstone_core_linked_share::run_cli_for_locale(
            locale,
            &args,
            Path::new(""),
            &|_: &str, _: &str| Err("help and usage read nothing".to_owned()),
        )
    } else {
        let journal = match resolve_process_journal_path() {
            Ok(journal) => journal.path,
            Err(error) => {
                eprint_journal_path_error_in(locale, error);
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let paths = match solstone_core_talent_runtime::resolve_runtime_paths() {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!(
                    "{}",
                    error
                        .context("journal.cli.share.startup.failed")
                        .render(locale)
                );
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let generate = match solstone_core_generate::OneShotClient::resolve_sibling() {
            Ok(generate) => generate.with_diagnostic_renderer(move |detail| {
                solstone_core_linked_share::transport_diagnostic(locale, detail)
            }),
            Err(error) => {
                let message = match error {
                    SiblingError::Current(error) => MessageDetail::message(
                        "journal.cli.share.startup.failed",
                        [("detail", &error)],
                    ),
                    SiblingError::NoParent => {
                        MessageDetail::message("journal.cli.share.startup.sibling.parent", [])
                    }
                    SiblingError::Missing(path) => MessageDetail::message(
                        "journal.cli.share.startup.sibling.missing",
                        [("path", &path.display())],
                    ),
                };
                eprintln!("{}", message.render(locale));
                return ExitCode::from(EXIT_TEMPFAIL);
            }
        };
        let talents = solstone_core_linked_share::RuntimeTalents::new(&journal, paths, &generate);
        solstone_core_linked_share::run_cli_for_locale(locale, &args, &journal, &talents)
    };
    print!("{}", run.stdout);
    eprint!("{}", run.stderr);
    ExitCode::from(u8::try_from(run.exit_code).unwrap_or(1))
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
        jiff::Timestamp::now(),
        &writer,
        &mut stdout.lock(),
        &mut stderr.lock(),
    );
    ExitCode::from(exit_code as u8)
}

fn run_backfill_facet_ids(commit: bool) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    match solstone_core_facets::backfill_facet_ids(&journal, commit) {
        Ok(report) => {
            let line = if commit {
                solstone_core_cli_copy::render_for(
                    host_locale(),
                    "journal.cli.host.backfill.facets.applied",
                    &[
                        (
                            "scanned",
                            CopyValue::String(&(report.total_scanned).to_string()),
                        ),
                        (
                            "backfilled",
                            CopyValue::String(&(report.backfilled_count).to_string()),
                        ),
                        (
                            "scanned_quantity",
                            CopyValue::Cardinal(host_count_quantity(report.total_scanned as u64)),
                        ),
                        (
                            "backfilled_quantity",
                            CopyValue::Cardinal(host_count_quantity(
                                report.backfilled_count as u64,
                            )),
                        ),
                    ],
                )
            } else {
                solstone_core_cli_copy::render_for(
                    host_locale(),
                    "journal.cli.host.backfill.facets.dry",
                    &[
                        (
                            "scanned",
                            CopyValue::String(&(report.total_scanned).to_string()),
                        ),
                        (
                            "backfilled",
                            CopyValue::String(&(report.backfilled_count).to_string()),
                        ),
                        (
                            "scanned_quantity",
                            CopyValue::Cardinal(host_count_quantity(report.total_scanned as u64)),
                        ),
                        (
                            "backfilled_quantity",
                            CopyValue::Cardinal(host_count_quantity(
                                report.backfilled_count as u64,
                            )),
                        ),
                    ],
                )
            };
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.backfill.facet.ids.solstone.journal.backfill.facet.ids.error",
                    [(
                        "error",
                        &solstone_core_facets::cli_copy::facet_id(host_locale(), &error)
                    )]
                )
            );
            ExitCode::from(EXIT_DATAERR)
        }
    }
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

#[cfg(any(unix, windows))]
fn run_convey(
    options: ConveyOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let journal = match resolve_journal_config_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    // Supervised Windows web helpers borrow the one supervisor generation.
    // Validate the selected journal before service admission or readiness; the
    // hosted runtime retains its own clone through every outstanding helper.
    #[cfg(windows)]
    let _speakers_generation = match solstone_core_transcribe::enter_speakers_analyze_generation(
        &journal,
        solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Convey,
        admitted,
    ) {
        Ok(generation) => generation,
        Err(error) => {
            if let Some(message) =
                solstone_core_transcribe::speakers_installation_detail_in(&error, host_locale())
            {
                eprintln!("{message}");
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let service_journal = journal.clone();
    run_hosted_service(
        &journal,
        HostedServiceKind::Convey,
        #[cfg(windows)]
        admitted,
        move |parent| {
            // Beside the server, so a long walk never delays it.
            let sweep_journal = service_journal.clone();
            let _ = std::thread::spawn(move || {
                solstone_core_import_host::strava_convert::sweep_strava_conversions(&sweep_journal);
            });
            #[cfg(feature = "journal-mcp-endpoint")]
            let agents_routes = Some(solstone_core_mcp_endpoint::owner_routes(
                service_journal.clone(),
            ));
            #[cfg(not(feature = "journal-mcp-endpoint"))]
            let agents_routes = None;
            match solstone_core_convey_shell::run_convey_with_hosted_parent_and_routes(
                service_journal,
                options.port,
                parent,
                agents_routes,
                #[cfg(windows)]
                &_speakers_generation.child_launch_context(),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        },
    )
}

#[cfg(not(any(unix, windows)))]
fn run_convey(
    _options: ConveyOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    unavailable()
}

fn run_schedule(_options: ScheduleOptions) -> ExitCode {
    let journal = match resolve_journal_config_path(None) {
        Ok(line) => line.path,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let report = solstone_core_system::schedule::build_schedule_report(
        journal.join("config/schedules.json"),
        journal.join("health/scheduler.json"),
        solstone_core_system::schedule::ScheduleNow::in_zone(
            jiff::Timestamp::now(),
            solstone_core_journal_config::owner_zone(&journal),
        ),
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
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let cluster_id = required_i64(object, "cluster_id")?;
    let request = IdentifyClusterRequest {
        journal_root: root,
        cluster_id,
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
    use solstone_core_speaker_resolve::backfill_coordinator::{
        StartBackfillRequest, backfill_status, start_backfill,
    };
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
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let operation_id = required_string(object, "operation_id")?;
    let reattribute = required_bool(object, "reattribute")?;
    let now_ms = required_i64(object, "now_ms")?;

    let mut status = start_backfill(
        &root,
        &StartBackfillRequest {
            operation_id: Some(operation_id.clone()),
            commit: true,
            reattribute,
            accumulation: false,
            now_ms,
        },
    )
    .map_err(|error| error.to_string())?;

    while !status.done {
        std::thread::sleep(std::time::Duration::from_millis(20));
        if let Some(s) = backfill_status(&root, &operation_id).map_err(|error| error.to_string())? {
            status = s;
        }
    }

    Ok(json!({
        "operation_id": status.operation_id,
        "total_count": status.total_count,
        "processed_count": status.completed_count,
        "skipped_count": status.protected_skipped,
        "error_count": status.error_count,
        "error_segments": status.error_segments.into_iter().map(|error| json!({
            "day": error.segment.day,
            "stream": error.segment.stream,
            "segment_key": error.segment.segment_key,
            "detail": error.detail,
        })).collect::<Vec<_>>(),
        "pending_count": status.pending_count,
        "done": status.done,
    }))
}

fn backfill_status_request(value: Value) -> Result<Value, String> {
    use solstone_core_speaker_resolve::backfill_coordinator::backfill_status;
    let request_object = request_object(
        value,
        "solstone-speaker-resolve-backfill-status-request-v1",
        &["schema", "journal_root", "operation_id"],
    )?;
    let object = &request_object;
    let root = PathBuf::from(required_string(object, "journal_root")?);
    let operation_id = required_string(object, "operation_id")?;
    let status = backfill_status(&root, &operation_id).map_err(|error| error.to_string())?;
    Ok(status.map_or_else(
        || json!({"status": "not_found", "operation_id": operation_id}),
        |status| {
            json!({
                "status": if status.done { "done" } else { "resumable" },
                "operation_id": operation_id,
                "total_count": status.total_count,
                "completed_count": status.completed_count,
                "pending_count": status.pending_count,
                "error_count": status.error_count,
                "error_segments": status.error_segments.into_iter().map(|error| json!({
                    "day": error.segment.day,
                    "stream": error.segment.stream,
                    "segment_key": error.segment.segment_key,
                    "detail": error.detail,
                })).collect::<Vec<_>>(),
                "done": status.done,
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
    use solstone_core_journal_io::SegmentLayout;
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
    let stream = string(segment, "stream")?;
    let stream_layout = match segment.get("stream_layout").and_then(Value::as_str) {
        Some("direct") => SegmentLayout::Direct,
        Some("named") => SegmentLayout::Named,
        Some(_) => return Err("invalid stream_layout".to_owned()),
        None => {
            if stream == solstone_core_journal_io::DEFAULT_STREAM {
                SegmentLayout::Direct
            } else {
                SegmentLayout::Named
            }
        }
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
        stream_layout,
        stream,
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
            "stream_layout",
        ],
    )?;
    let root = PathBuf::from(required_string(&object, "journal_root")?);
    let stream_layout = match object.get("stream_layout").and_then(Value::as_str) {
        Some("direct") => Some(solstone_core_journal_io::SegmentLayout::Direct),
        Some("named") => Some(solstone_core_journal_io::SegmentLayout::Named),
        Some(_) => return Err("invalid stream_layout".to_owned()),
        None => None,
    };
    let probe = ContaminationProbe {
        day: required_string(&object, "day")?,
        stream_layout,
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
    require_person_speakers(&object, labels.iter().map(|label| label.get("speaker")))?;
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
    require_person_speakers(
        &object,
        patches.iter().map(|(_, fields)| fields.get("speaker")),
    )?;
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
    let correction = required_object_value(&object, "correction")?;
    require_person_speakers(&object, [correction.get("corrected_speaker")])?;
    solstone_core_speaker_id::corrections::append_correction(&segment_dir(&object)?, correction)
        .map_err(|error| error.to_string())?;
    Ok(json!({"status":"appended"}))
}

fn wipe_speaker_artifacts_request(value: Value) -> Result<Value, String> {
    let object = request_object(
        value,
        "solstone-speaker-resolve-wipe-speaker-artifacts-request-v1",
        &["schema", "journal_root", "dry_run", "erase"],
    )?;
    let journal_root = PathBuf::from(required_string(&object, "journal_root")?);
    if required_bool(&object, "dry_run")? {
        let report = solstone_core_speaker_resolve::artifact_wipe::dry_run(&journal_root)
            .map_err(|error| error.to_string())?;
        return serde_json::to_value(report).map_err(|error| error.to_string());
    }
    // Moved to the trash; erased at once only when asked.
    let erase = match object.get("erase") {
        None => false,
        Some(Value::Bool(erase)) => *erase,
        Some(_) => return Err("erase must be a boolean".to_owned()),
    };
    solstone_core_speaker_resolve::speakers_trash::wipe(
        &journal_root,
        solstone_core_removals::Surface::Cli,
        erase,
    )
    .map(|wiped| wiped.to_json())
    .map_err(|error| error.to_string())
}

/// Refuse a label or correction that names a speaker who is not an admissible
/// person. An absent or null speaker names no one and passes.
fn require_person_speakers<'a>(
    object: &Map<String, Value>,
    speakers: impl IntoIterator<Item = Option<&'a Value>>,
) -> Result<(), String> {
    let named = speakers
        .into_iter()
        .flatten()
        .filter(|speaker| !speaker.is_null())
        .collect::<Vec<_>>();
    if named.is_empty() {
        return Ok(());
    }
    let journal_root = PathBuf::from(required_string(object, "journal_root")?);
    let people = solstone_core_entity::load_all_journal_entities(&journal_root)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(solstone_core_entity::is_admissible_person)
        .map(|entity| entity.id)
        .collect::<std::collections::HashSet<_>>();
    for speaker in named {
        match speaker.as_str() {
            Some(id) if people.contains(id) => {}
            _ => return Err(format!("speaker {speaker} is not a person in this journal")),
        }
    }
    Ok(())
}

fn segment_dir(object: &Map<String, Value>) -> Result<PathBuf, String> {
    use solstone_core_journal_io::SegmentLayout;

    let root = PathBuf::from(required_string(object, "journal_root")?);
    let segment = object
        .get("segment")
        .and_then(Value::as_object)
        .ok_or_else(|| "segment is required".to_owned())?;
    const FIELDS: [&str; 4] = ["day", "stream_layout", "stream", "segment_key"];
    if segment.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err("invalid segment request".to_owned());
    }
    let day = required_string(segment, "day")?;
    let stream = required_string(segment, "stream")?;
    let segment_key = required_string(segment, "segment_key")?;
    let stream_layout = match segment.get("stream_layout").and_then(Value::as_str) {
        Some("direct") => SegmentLayout::Direct,
        Some("named") => SegmentLayout::Named,
        Some(_) => return Err("invalid stream_layout".to_owned()),
        None => {
            if stream == solstone_core_journal_io::DEFAULT_STREAM {
                SegmentLayout::Direct
            } else {
                SegmentLayout::Named
            }
        }
    };
    let path = match stream_layout {
        SegmentLayout::Direct => format!("chronicle/{day}/{segment_key}"),
        SegmentLayout::Named => format!("chronicle/{day}/{stream}/{segment_key}"),
    };
    solstone_core_journal_io::contained_path(&root, &path).map_err(|error| error.to_string())
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
    match solstone_core_transcribe::publish_transcript_request(&input) {
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
        | SpeakerTranscriptWriteError::Internal { .. }
        | SpeakerTranscriptWriteError::Cancelled => EXIT_TEMPFAIL,
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
        LocalCommand::Connect => run_local_connect(),
        LocalCommand::Install(command) => run_local_install(command),
        LocalCommand::Generate => run_local_generate(),
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
    solstone_core_generate_wire::respond(
        request,
        endpoint_runtime,
        journal,
        acquire_local_inference_authority,
        &solstone_core_generate::Cancellation::never(),
    )
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
    // the binary can decide: journal resolution and what the process does when
    // the protocol cannot continue. Provider dispatch is shared with app hosts.
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

fn acquire_local_inference_authority(
    journal: &Path,
) -> Option<solstone_core_local::LocalInferenceAuthority> {
    // The operation establishes its deadline before fetching its snapshot.
    solstone_core_callosum::local_inference_refresh(journal).map(|refresh| {
        solstone_core_local::LocalInferenceAuthority::new(0, 0, String::new(), Some(refresh))
    })
}

fn run_local_connect() -> ExitCode {
    let input: solstone_core_local::ConnectInput = match read_local_stdin(MAX_LOCAL_STDIN_BYTES) {
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
    let mut authority = acquire_local_inference_authority(Path::new(&input.journal_path));
    let outcome = solstone_core_local::connect_with_authority(input, authority.as_mut());
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &outcome).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn run_local_generate() -> ExitCode {
    let input: solstone_core_local::GenerateInput =
        match read_local_stdin(MAX_LOCAL_GENERATE_STDIN_BYTES) {
            Ok(input) => input,
            Err(LocalStdinError::Content) => {
                eprintln!("local command failed: stdin was not valid JSON within 64 MiB");
                return ExitCode::from(EXIT_USAGE);
            }
            Err(LocalStdinError::Io) => {
                eprintln!("local command failed: stdin I/O error");
                return ExitCode::from(EXIT_IOERR);
            }
        };
    // A direct local call takes its turn in the journal's one call line too.
    let priority =
        match solstone_core_journal_config::work_priority::WorkPriority::from_environment() {
            Ok(priority) => priority,
            Err(detail) => {
                eprintln!("local command failed: {detail}");
                return ExitCode::from(EXIT_USAGE);
            }
        };
    let _turn = match solstone_core_local::admission::wait_for_model_call(
        Path::new(&input.journal_path),
        priority,
        &|| false,
    ) {
        Ok(turn) => Some(turn),
        // The line orders calls; a journal whose line cannot open still calls.
        Err(error) => {
            eprintln!("model call line is unavailable, so this call runs unordered: {error:?}");
            None
        }
    };
    let mut authority = acquire_local_inference_authority(Path::new(&input.journal_path));
    let outcome = solstone_core_local::generate_with_authority(input, authority.as_mut());
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &outcome).is_err() || writeln!(stdout).is_err() {
        return ExitCode::from(EXIT_IOERR);
    }
    ExitCode::SUCCESS
}

fn run_local_json<T, O>(operation: impl FnOnce(T) -> O) -> ExitCode
where
    T: DeserializeOwned,
    O: serde::Serialize,
{
    run_local_json_with_limit(operation, MAX_LOCAL_STDIN_BYTES, "1 MiB")
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
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut context = match solstone_core_doctor::context::CheckContext::production(args.port) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{}", solstone_core_doctor::context_error(&error));
            return ExitCode::from(1);
        }
    };
    #[cfg(windows)]
    {
        context.windows_service_probe = Some(service_windows::doctor_registration);
    }
    let started =
        solstone_core_time::utc_z(jiff::Timestamp::now(), solstone_core_time::Precision::Secs);
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
        solstone_core_brain::inspect_brain_state(&line.path, &config, jiff::Timestamp::now());
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
        Some(journal_path),
    )
}

fn brain_fingerprint_result(
    config: &Map<String, Value>,
    hmac_key: Option<&[u8]>,
    bundled_runtime_fingerprint_sha256: Option<String>,
    journal: Option<&std::path::Path>,
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
            journal,
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
    let journal = resolve_process_journal_path().ok();
    let output = brain_fingerprint_result(
        &request.config,
        Some(&request.hmac_key),
        request.bundled_runtime_fingerprint_sha256,
        journal.as_ref().map(|line| line.path.as_path()),
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
        jiff::Timestamp::now(),
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
            eprintln!("thinking check failed: {error}");
            return ExitCode::from(EXIT_DATAERR);
        }
        Err(solstone_core_brain::BeginRefreshError::InvalidArgument(error)) => {
            eprintln!("thinking check failed: {error}");
            return ExitCode::from(EXIT_USAGE);
        }
        Err(solstone_core_brain::BeginRefreshError::Writer(error)) => {
            eprintln!("thinking check failed: {error}");
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
        jiff::Timestamp::now(),
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
            jiff::Timestamp::now(),
            bundled_runtime_fingerprint_sha256,
        ) {
            Ok(_) => write_refresh_projection(&journal_path),
            Err(error) => {
                eprintln!("thinking check failed: {error}");
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
                jiff::Timestamp::now(),
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
        match parse_refresh_session_record(&line, jiff::Timestamp::now()) {
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
        match parse_prerequisite_renewal_session_record(&line, jiff::Timestamp::now()) {
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
    now: jiff::Timestamp,
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
    now: jiff::Timestamp,
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
        jiff::Timestamp::now(),
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
        jiff::Timestamp::now(),
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
        jiff::Timestamp::now(),
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
        jiff::Timestamp::now(),
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
            eprintln!("thinking check failed: could not read journal config: {error}");
            return ExitCode::from(EXIT_UNAVAILABLE);
        }
    };
    let projection =
        solstone_core_brain::inspect_brain_state(journal_path, &config, jiff::Timestamp::now())
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
        eprintln!("thinking check failed: stdout I/O error");
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
        jiff::Timestamp::now(),
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

fn run_spl_process(
    command: SplCommand,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    match command {
        SplCommand::Service(options) => run_spl_service(
            options,
            #[cfg(windows)]
            admitted,
        ),
    }
}

fn run_mcp_process(
    command: McpCommand,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    match command {
        McpCommand::Service => run_mcp_service(
            #[cfg(windows)]
            admitted,
        ),
        McpCommand::LocalDoor => run_mcp_local_door(
            #[cfg(windows)]
            admitted,
        ),
        McpCommand::Status => run_mcp_status(),
        McpCommand::Token(command) => run_mcp_token(command),
        McpCommand::Pairing(command) => run_mcp_pairing(command),
        McpCommand::Oauth(command) => run_mcp_oauth(command),
        McpCommand::Permission(command) => run_mcp_permission(command),
        McpCommand::Activity(command) => run_mcp_activity(command),
        McpCommand::Probe(command) => run_mcp_probe_command(command),
    }
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_service(
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.path.clone();
    run_hosted_service(
        &journal.path,
        HostedServiceKind::Mcp,
        #[cfg(windows)]
        admitted,
        move |parent| match solstone_core::run_native_service_with_hosted_parent(
            service_journal,
            parent,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.service.mcp.service.failed",
                        [("class", &(error.class()))]
                    )
                );
                ExitCode::from(EXIT_TEMPFAIL)
            }
        },
    )
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_service(
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    // This build has no MCP endpoint, so the admitted launch has nothing to drive.
    #[cfg(windows)]
    let _ = admitted;
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.service.service.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_local_door(
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.path.clone();
    run_hosted_service(
        &journal.path,
        HostedServiceKind::McpLocalDoor,
        #[cfg(windows)]
        admitted,
        move |parent| match solstone_core::run_local_door_with_hosted_parent(
            service_journal,
            parent,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.local.door.mcp.local.door.failed",
                        [("class", &(error.class()))]
                    )
                );
                ExitCode::from(EXIT_TEMPFAIL)
            }
        },
    )
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_local_door(
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    // This build has no MCP endpoint, so the admitted launch has nothing to drive.
    #[cfg(windows)]
    let _ = admitted;
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.local.door.local.door.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_status() -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };

    let mut exit = ExitCode::SUCCESS;
    let config_read = read_journal_config(&journal.path);
    let capability = match &config_read {
        Ok(read) => match mcp_endpoint_capability(read) {
            Ok(McpEndpointCapability::Enabled) => {
                host_text("journal.cli.host.mcp.status.capability.enabled")
            }
            Ok(McpEndpointCapability::Disabled) => {
                host_text("journal.cli.host.mcp.status.capability.disabled")
            }
            Err(error) => {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.status.status.mcp.capability.configuration.is.invalid",
                        [("error", &(format!("{:?}", error)))]
                    )
                );
                exit = ExitCode::from(EXIT_UNAVAILABLE);
                host_text("journal.cli.host.mcp.status.capability.error")
            }
        },
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.mcp.status.status.could.not.read.mcp.capability.configuration",
                    [("error", &error.render(host_locale()))]
                )
            );
            exit = ExitCode::from(EXIT_UNAVAILABLE);
            host_text("journal.cli.host.mcp.status.capability.error")
        }
    };

    let local_door_enabled = config_read
        .as_ref()
        .is_ok_and(solstone_core_journal_config::local_door_enabled);
    let lan_door_enabled = config_read
        .as_ref()
        .is_ok_and(solstone_core_journal_config::lan_door_enabled);
    let token_count = match TokenStore::open(&journal.path).list() {
        Ok(tokens) => host_display(
            "journal.cli.host.mcp.status.token.count",
            [("count", &(tokens.len()))],
        ),
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.mcp.status.status.could.not.read.bearer.token.ledger",
                    [("error", &error.message_for(host_locale()))]
                )
            );
            exit = ExitCode::from(token_store_error_exit(&error));
            host_text("journal.cli.host.mcp.status.token.unavailable")
        }
    };

    println!(
        "{}",
        host_text(
            "journal.cli.host.mcp.status.mcp.capability.config.status.only.this.does.not.report.service"
        )
    );
    println!(
        "{}",
        host_text("journal.cli.host.mcp.status.capability.compiled.in.true")
    );
    println!("{capability}");
    println!(
        "{}",
        if local_door_enabled {
            host_display(
                "journal.cli.host.mcp.status.local.enabled",
                [(
                    "resource",
                    &(solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE),
                )],
            )
        } else {
            host_display(
                "journal.cli.host.mcp.status.local.disabled",
                [(
                    "resource",
                    &(solstone_core_journal_config::MCP_LOCAL_DOOR_RESOURCE),
                )],
            )
        }
    );
    println!(
        "{}",
        if lan_door_enabled {
            host_display(
                "journal.cli.host.mcp.status.lan.enabled",
                [("port", &(solstone_core_journal_config::MCP_LAN_DOOR_PORT))],
            )
        } else {
            host_display(
                "journal.cli.host.mcp.status.lan.disabled",
                [("port", &(solstone_core_journal_config::MCP_LAN_DOOR_PORT))],
            )
        }
    );
    println!("{token_count}");
    exit
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_status() -> ExitCode {
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.status.status.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(feature = "journal-mcp-endpoint")]
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
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.token.created.bearer.token.for.label",
                        [("label", &(format!("{:?}", created.label)))]
                    )
                );
                println!(
                    "{}",
                    host_text(
                        "journal.cli.host.mcp.token.save.this.token.now.it.will.not.be.shown.again"
                    )
                );
                println!("{}", created.token);
                println!();
                println!(
                    "{}",
                    host_text(
                        "!journal.cli.host.mcp.token.until.you.grant.it.read.access.an.agent.using.this"
                    )
                );
                let categories = solstone_core_format::content::kinds::default_permission_words()
                    .map(|word| format!(" --category {word}"))
                    .collect::<String>();
                println!(
                    "  solstone journal mcp permission set --token {:?}{categories}",
                    created.label
                );
                println!(
                    "{}",
                    host_text(
                        "!journal.cli.host.mcp.token.grant.before.the.agent.connects.an.agent.that.connected.first"
                    )
                );
                ExitCode::SUCCESS
            }
            Err(error) => render_token_store_error("create", &error),
        },
        McpTokenCommand::List => match store.list() {
            Ok(tokens) => {
                for token in tokens {
                    println!(
                        "{}\t{}",
                        token.label,
                        solstone_core_time::utc_rfc3339(token.created_at)
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => render_token_store_error("list", &error),
        },
        McpTokenCommand::Revoke { label } => match store.revoke(&label) {
            Ok(()) => {
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.token.revoked.bearer.token.for.label",
                        [("label", &(format!("{:?}", label)))]
                    )
                );
                ExitCode::SUCCESS
            }
            Err(error) => render_token_store_error("revoke", &error),
        },
    }
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_token(_command: McpTokenCommand) -> ExitCode {
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.token.token.management.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_pairing(command: McpPairingCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    run_mcp_pairing_at(&journal.path, command)
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_pairing_at(journal_root: &Path, command: McpPairingCommand) -> ExitCode {
    let store = OAuthStore::open(journal_root);
    match command {
        McpPairingCommand::Generate { door } => {
            let mint_symbol = match door.as_deref() {
                None | Some("local") => "local",
                Some("lan") => "lan",
                Some("solstone.me") => "relay",
                Some("hostname") => {
                    let config_read = match read_journal_config(journal_root) {
                        Ok(cfg) => cfg,
                        Err(_) => {
                            eprintln!(
                                "{}",
                                host_text(
                                    "journal.cli.host.mcp.pairing.at.pairing.generate.this.journal.s.config.could.not.be.read"
                                )
                            );
                            return ExitCode::from(EXIT_NOINPUT);
                        }
                    };
                    match solstone_core_journal_config::byo_hostname_config(&config_read) {
                        solstone_core_journal_config::ByoHostnameConfigStatus::Invalid => {
                            eprintln!(
                                "{}",
                                host_text(
                                    "journal.cli.host.mcp.pairing.at.pairing.generate.this.journal.s.config.could.not.be.read"
                                )
                            );
                            return ExitCode::from(EXIT_NOINPUT);
                        }
                        solstone_core_journal_config::ByoHostnameConfigStatus::None
                        | solstone_core_journal_config::ByoHostnameConfigStatus::Configured(
                            solstone_core_journal_config::ByoHostnameConfig {
                                hostname: None, ..
                            },
                        ) => {
                            eprintln!(
                                "{}",
                                host_text(
                                    "journal.cli.host.mcp.pairing.at.pairing.generate.no.hostname.is.set.up.for.this.journal"
                                )
                            );
                            return ExitCode::from(EXIT_NOHOST);
                        }
                        solstone_core_journal_config::ByoHostnameConfigStatus::Configured(
                            solstone_core_journal_config::ByoHostnameConfig {
                                hostname: Some(_),
                                ..
                            },
                        ) => "byo",
                    }
                }
                Some(other) => other,
            };

            match store.generate_pairing_code_with_door(mint_symbol) {
                Ok(created) => {
                    println!("{}", host_text("journal.cli.host.mcp.pairing.generated"));
                    match door.as_deref() {
                        None => {
                            println!(
                                "{}",
                                host_text(
                                    "!journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer.to"
                                )
                            );
                        }
                        Some("local") => {
                            println!(
                                "{}",
                                host_text(
                                    "!journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer"
                                )
                            );
                        }
                        Some("lan") => {
                            println!(
                                "{}",
                                host_text(
                                    "!journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.your.network"
                                )
                            );
                        }
                        Some("solstone.me") => {
                            println!(
                                "{}",
                                host_text(
                                    "!journal.cli.host.mcp.pairing.at.it.works.only.through.solstone.me"
                                )
                            );
                        }
                        Some("hostname") => {
                            println!(
                                "{}",
                                host_text(
                                    "!journal.cli.host.mcp.pairing.at.it.works.only.at.your.own.domain"
                                )
                            );
                        }
                        Some(_) => {
                            println!(
                                "{}",
                                host_text(
                                    "!journal.cli.host.mcp.pairing.at.it.works.only.for.an.agent.on.this.computer"
                                )
                            );
                        }
                    }
                    println!(
                        "{}",
                        host_text(
                            "journal.cli.host.mcp.pairing.at.save.this.now.it.will.not.be.shown.again.and"
                        )
                    );
                    println!("{}", created.code);
                    println!(
                        "{}",
                        host_display(
                            "journal.cli.host.mcp.pairing.at.expires.at",
                            [(
                                "expires_at",
                                &(solstone_core_time::utc_rfc3339(created.expires_at))
                            )]
                        )
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => render_oauth_store_error("pairing", "generate", &error),
            }
        }
        McpPairingCommand::Revoke => match store.revoke_pairing_code() {
            Ok(()) => {
                println!(
                    "{}",
                    host_text("journal.cli.host.mcp.pairing.at.revoked.the.active.pairing.code")
                );
                ExitCode::SUCCESS
            }
            Err(OAuthStoreError::NoActivePairing) => {
                eprintln!(
                    "{}",
                    host_text(
                        "journal.cli.host.mcp.pairing.at.pairing.revoke.no.active.pairing.code.to.revoke"
                    )
                );
                ExitCode::from(EXIT_DATAERR)
            }
            Err(error) => render_oauth_store_error("pairing", "revoke", &error),
        },
    }
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_pairing(_command: McpPairingCommand) -> ExitCode {
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.pairing.pairing.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(feature = "journal-mcp-endpoint")]
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
                        solstone_core_time::utc_rfc3339(client.created_at)
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => render_oauth_store_error("oauth", "list", &error),
        },
        McpOauthCommand::Revoke { client_id } => match store.revoke_client_by_client_id(&client_id)
        {
            Ok(()) => {
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.oauth.revoked.oauth.client",
                        [("client_id", &(client_id))]
                    )
                );
                ExitCode::SUCCESS
            }
            Err(OAuthStoreError::ClientNotFound) => {
                eprintln!(
                    "{}",
                    host_text("journal.cli.host.mcp.oauth.oauth.revoke.no.such.oauth.client")
                );
                ExitCode::from(EXIT_DATAERR)
            }
            Err(error) => render_oauth_store_error("oauth", "revoke", &error),
        },
    }
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_oauth(_command: McpOauthCommand) -> ExitCode {
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.oauth.oauth.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(feature = "journal-mcp-endpoint")]
fn render_token_store_error(operation: &str, error: &TokenStoreError) -> ExitCode {
    match error {
        TokenStoreError::InvalidLabel(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.token.store.error.token.invalid.label",
                    [
                        ("operation", &(operation)),
                        ("error", &error.message_for(host_locale()))
                    ]
                )
            );
        }
        TokenStoreError::DuplicateLabel { label } => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.token.store.error.token.a.bearer.token.already.exists.for.label",
                    [
                        ("operation", &(operation)),
                        ("label", &(format!("{:?}", label)))
                    ]
                )
            );
        }
        TokenStoreError::NotFound { label } => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.token.store.error.token.no.bearer.token.exists.for.label",
                    [
                        ("operation", &(operation)),
                        ("label", &(format!("{:?}", label)))
                    ]
                )
            );
        }
        _ => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.token.store.error.token",
                    [
                        ("operation", &(operation)),
                        ("error", &error.message_for(host_locale()))
                    ]
                )
            );
        }
    }
    ExitCode::from(token_store_error_exit(error))
}

#[cfg(feature = "journal-mcp-endpoint")]
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

#[cfg(feature = "journal-mcp-endpoint")]
fn render_oauth_store_error(command: &str, operation: &str, error: &OAuthStoreError) -> ExitCode {
    match error {
        OAuthStoreError::NoActivePairing => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.oauth.store.error.no.active.pairing.code",
                    [("command", &(command)), ("operation", &(operation))]
                )
            );
        }
        OAuthStoreError::ClientNotFound => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.render.oauth.store.error.no.such.oauth.client",
                    [("command", &(command)), ("operation", &(operation))]
                )
            );
        }
        _ => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.mcp.oauth.error.detail",
                    [
                        ("command", &(command)),
                        ("operation", &(operation)),
                        ("error", &error.message_for(host_locale()))
                    ]
                )
            );
        }
    }
    ExitCode::from(oauth_store_error_exit(error))
}

#[cfg(feature = "journal-mcp-endpoint")]
const fn oauth_store_error_exit(error: &OAuthStoreError) -> u8 {
    match error {
        OAuthStoreError::NoActivePairing
        | OAuthStoreError::ClientNotFound
        | OAuthStoreError::InvalidToken
        | OAuthStoreError::InvalidGrant
        | OAuthStoreError::RefreshReused { .. }
        | OAuthStoreError::TransactionNotFound
        | OAuthStoreError::TransactionExpired
        | OAuthStoreError::TransactionExhausted
        | OAuthStoreError::CodeExpired
        | OAuthStoreError::BindingMismatch
        | OAuthStoreError::PairingMismatch
        | OAuthStoreError::PairingLocked => EXIT_DATAERR,
        OAuthStoreError::Lock(_) => EXIT_TEMPFAIL,
        OAuthStoreError::Directory(_) | OAuthStoreError::Read { .. } => EXIT_IOERR,
        OAuthStoreError::Write(_) | OAuthStoreError::Permission => EXIT_CANTCREAT,
        OAuthStoreError::Randomness
        | OAuthStoreError::Quota
        | OAuthStoreError::Malformed { .. }
        | OAuthStoreError::UnsupportedSchema { .. }
        | OAuthStoreError::EntryTooLarge
        | OAuthStoreError::StateTooLarge => EXIT_UNAVAILABLE,
    }
}

#[cfg(feature = "journal-mcp-endpoint")]
fn permission_facet_names(journal: &Path, ids: &[String]) -> String {
    ids.iter()
        .map(|id| {
            solstone_core_facets::resolve_facet_id(journal, id).unwrap_or_else(|_| id.clone())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_permission(command: McpPermissionCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let store = solstone_core::PermissionStore::open(&journal.path);
    match command {
        McpPermissionCommand::Show { target } => match target {
            None => match store.read() {
                Ok(file) => {
                    if file.permissions.is_empty() {
                        println!(
                            "{}",
                            host_text(
                                "journal.cli.host.mcp.permission.no.mcp.connection.permissions.configured"
                            )
                        );
                        return ExitCode::SUCCESS;
                    }
                    for perm in file.permissions {
                        let row = match perm.read.as_ref().map(|read| &read.scope) {
                            Some(solstone_core::ReadScope::WholeJournal) => host_display(
                                "!journal.cli.host.mcp.permission.list.whole",
                                [
                                    ("connection", &(perm.connection)),
                                    ("generation", &(perm.generation)),
                                    ("evaluation", &(perm.evaluation)),
                                ],
                            ),
                            Some(solstone_core::ReadScope::Facets { ids }) => host_display(
                                "!journal.cli.host.mcp.permission.list.facets",
                                [
                                    ("connection", &(perm.connection)),
                                    ("generation", &(perm.generation)),
                                    ("evaluation", &(perm.evaluation)),
                                    ("facets", &(permission_facet_names(&journal.path, ids))),
                                ],
                            ),
                            None => host_display(
                                "!journal.cli.host.mcp.permission.list.none",
                                [
                                    ("connection", &(perm.connection)),
                                    ("generation", &(perm.generation)),
                                    ("evaluation", &(perm.evaluation)),
                                ],
                            ),
                        };
                        println!("{row}");
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.mcp.permission.permission.show",
                            [("error", &error.message_for(host_locale()))]
                        )
                    );
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            },
            Some(target) => {
                let key = match resolve_connection_key(&journal.path, &target) {
                    Ok(k) => k,
                    Err(code) => return code,
                };
                match store.get_permission(&key) {
                    Ok(Some(perm)) => {
                        println!(
                            "{}",
                            host_display(
                                "journal.cli.host.mcp.permission.connection",
                                [("connection", &(perm.connection))]
                            )
                        );
                        println!(
                            "{}",
                            host_display(
                                "journal.cli.host.mcp.permission.generation",
                                [("generation", &(perm.generation))]
                            )
                        );
                        println!(
                            "{}",
                            host_display(
                                "journal.cli.host.mcp.permission.evaluation",
                                [("evaluation", &(perm.evaluation))]
                            )
                        );
                        match perm.read {
                            Some(read) => {
                                let scope = match &read.scope {
                                    solstone_core::ReadScope::WholeJournal => {
                                        host_text("!journal.cli.host.mcp.permission.scope.whole")
                                    }
                                    solstone_core::ReadScope::Facets { ids } => host_display(
                                        "!journal.cli.host.mcp.permission.scope.facets",
                                        [("facets", &(permission_facet_names(&journal.path, ids)))],
                                    ),
                                };
                                println!("{scope}");
                                let kinds = read.categories.iter().map(|word| {
                                    solstone_core_format::content::AdmittedCategory::from_stored_word(word)
                                        .map_or_else(|| word.clone(), |category| solstone_core_cli::permission_kind_label(category.kind_spec()))
                                }).collect::<Vec<_>>();
                                println!(
                                    "{}",
                                    host_display(
                                        "!journal.cli.host.mcp.permission.categories",
                                        [("categories", &(kinds.join(", ")))]
                                    )
                                );
                            }
                            None => {
                                println!(
                                    "{}",
                                    host_text(
                                        "!journal.cli.host.mcp.permission.read.no.read.permission.configured"
                                    )
                                );
                            }
                        }
                        ExitCode::SUCCESS
                    }
                    Ok(None) => {
                        println!(
                            "{}",
                            host_display(
                                "journal.cli.host.mcp.permission.connection",
                                [("connection", &(key))]
                            )
                        );
                        println!(
                            "{}",
                            host_text(
                                "!journal.cli.host.mcp.permission.permission.no.permission.configured"
                            )
                        );
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!(
                            "{}",
                            host_display(
                                "journal.cli.host.mcp.permission.permission.show",
                                [("error", &error.message_for(host_locale()))]
                            )
                        );
                        ExitCode::from(EXIT_TEMPFAIL)
                    }
                }
            }
        },
        McpPermissionCommand::Set {
            target,
            categories,
            facets,
        } => {
            let key = match resolve_connection_key(&journal.path, &target) {
                Ok(k) => k,
                Err(code) => return code,
            };
            if categories.iter().any(|category| {
                !solstone_core_format::content::AdmittedCategory::from_stored_word(category)
                    .is_some()
            }) {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.permission.permission.set.choose.a.category.from",
                        [(
                            "categories",
                            &(solstone_core_format::content::kinds::permission_words()
                                .collect::<Vec<_>>()
                                .join(", "))
                        )]
                    )
                );
                return ExitCode::from(EXIT_DATAERR);
            }
            let ids = match solstone_core::resolve_permission_facet_names_for(
                host_locale(),
                &journal.path,
                &facets,
            ) {
                Ok(ids) => ids,
                Err(message) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.mcp.permission.permission.set",
                            [("error", &(message))]
                        )
                    );
                    return ExitCode::from(EXIT_DATAERR);
                }
            };
            let mut categories = categories;
            solstone_core_format::content::kinds::record_profile_pics_choice(&mut categories);
            let perm = solstone_core::ReadPermission {
                categories,
                scope: if ids.is_empty() {
                    solstone_core::ReadScope::WholeJournal
                } else {
                    solstone_core::ReadScope::Facets { ids }
                },
            };
            match store.set_permission(&key, perm) {
                Ok(record) => {
                    println!(
                        "{}",
                        host_display(
                            "!journal.cli.host.mcp.permission.set.mcp.read.permission.for.connection.generation",
                            [
                                ("connection", &(record.connection)),
                                ("generation", &(record.generation))
                            ]
                        )
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.mcp.permission.permission.set",
                            [("error", &error.message_for(host_locale()))]
                        )
                    );
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        }
        McpPermissionCommand::Clear { target } => {
            let key = match resolve_connection_key(&journal.path, &target) {
                Ok(k) => k,
                Err(code) => return code,
            };
            match store.clear_permission(&key) {
                Ok(cleared) => {
                    if cleared {
                        println!(
                            "{}",
                            host_display(
                                "!journal.cli.host.mcp.permission.cleared.permission.for.connection",
                                [("connection", &(key))]
                            )
                        );
                    } else {
                        println!(
                            "{}",
                            host_display(
                                "journal.cli.host.mcp.permission.no.permission.was.configured.for.connection",
                                [("connection", &(key))]
                            )
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.mcp.permission.permission.clear",
                            [("error", &error.message_for(host_locale()))]
                        )
                    );
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        }
    }
}

/// Show what each connection asked for and how it ended.
///
/// ⛔ Owner-only: this reads the journal directly, on the owner's machine, and
/// no MCP bearer credential reaches it — the closed tool registry has no row
/// that names this surface. ⚠ Operator-plain by design; the owner's product
/// surface and its wording are out of scope here.
#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_activity(command: solstone_core_cli::McpActivityCommand) -> ExitCode {
    use solstone_core::{ActivityQuery, RecordedOutcome, read_activity_for_locale, tally};

    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let connection = match command.target.as_ref() {
        Some(target) => match resolve_connection_key(&journal.path, target) {
            Ok(key) => Some(key),
            Err(code) => return code,
        },
        None => None,
    };
    let tool = match command.tool.as_deref() {
        Some(value) => match solstone_core_mcp_audit_tool_name(value) {
            Some(tool) => Some(tool),
            None => {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.activity.activity.unknown.tool",
                        [("value", &(format!("{:?}", value)))]
                    )
                );
                return ExitCode::from(EXIT_DATAERR);
            }
        },
        None => None,
    };
    let outcome = match command.outcome.as_deref() {
        Some(value) => match RecordedOutcome::from_token(value) {
            Some(outcome) => Some(outcome),
            None => {
                eprintln!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.activity.activity.unknown.outcome",
                        [("value", &(format!("{:?}", value)))]
                    )
                );
                return ExitCode::from(EXIT_DATAERR);
            }
        },
        None => None,
    };

    let page = match read_activity_for_locale(
        &journal.path,
        &ActivityQuery {
            connection,
            tool,
            outcome,
            day_from: command.day_from.clone(),
            day_to: command.day_to.clone(),
            limit: command.limit,
            start_after: None,
        },
        host_locale(),
    ) {
        Ok(page) => page,
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.mcp.activity.activity",
                    [("error", &error.message_for(host_locale()))]
                )
            );
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };

    if command.json {
        let entries = page
            .entries
            .iter()
            .map(|entry| {
                let result = entry.result.as_ref().map(|result| {
                    let mut output = serde_json::json!({
                        "count": result.count,
                        "targets": result.targets,
                        "targets_truncated": result.targets_truncated,
                        "digest": result.digest,
                    });
                    if let Some(origin) = result.origin.as_ref() {
                        output["origin"] = origin.clone();
                    }
                    if let Some(created_at) = result.created_at.as_ref() {
                        output["created_at"] =
                            serde_json::json!(solstone_core_time::utc_iso(*created_at));
                    }
                    if let Some(byte_count) = result.byte_count {
                        output["byte_count"] = serde_json::json!(byte_count);
                    }
                    output
                });
                serde_json::json!({
                    "day": entry.day,
                    "segment": entry.segment,
                    "schema": entry.schema,
                    "timestamp": solstone_core_time::utc_iso(entry.timestamp),
                    "connection": entry.connection,
                    "agent_identity": entry.agent_identity,
                    "tool": entry.tool_name.token(),
                    "outcome": entry.outcome.token(),
                    "reason": entry.reason,
                    "request": entry.request.as_ref().map(|request| serde_json::json!({
                        "arguments": request.arguments,
                        "arguments_omitted": request.arguments_omitted,
                    })),
                    "result": result,
                })
            })
            .collect::<Vec<_>>();
        let document = serde_json::json!({
            "entries": entries,
            "examined": page.examined,
            "examination_complete": page.examination_complete,
            "unreadable": page.unreadable,
        });
        println!("{document}");
        return ExitCode::SUCCESS;
    }

    if page.entries.is_empty() {
        // ⚠ Nothing found and the walk hitting its bound are different
        // answers, and only one of them means "there is none".
        if page.examination_complete {
            println!(
                "{}",
                host_text("journal.cli.host.mcp.activity.no.recorded.mcp.activity.matches")
            );
        } else {
            println!(
                "{}",
                solstone_core_cli_copy::render_for(
                    host_locale(),
                    "journal.cli.host.mcp.activity.no.match",
                    &[
                        ("examined", CopyValue::String(&(page.examined).to_string())),
                        (
                            "examined_quantity",
                            CopyValue::Cardinal(host_count_quantity(page.examined as u64))
                        )
                    ]
                )
            );
        }
        return ExitCode::SUCCESS;
    }

    for entry in &page.entries {
        println!(
            "{}\t{}\t{}\t{}",
            entry.timestamp.strftime("%Y-%m-%dT%H:%M:%SZ"),
            entry.outcome.token(),
            entry.tool_name.token(),
            entry
                .connection
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| host_text("journal.cli.host.mcp.activity.schema.connection")),
        );
        // ⚠ The coordinate is deliberately not beside the timestamp. The
        // journal's allocator takes a random ±1-second step to deconflict a
        // colliding segment name, so the two legitimately disagree by a second
        // or two and reading them side by side looks like a bug in the log.
        println!(
            "{}",
            host_display(
                "journal.cli.host.mcp.activity.record",
                [("day", &(entry.day)), ("segment", &(entry.segment))]
            )
        );
        match entry.request.as_ref() {
            Some(request) if request.arguments_omitted => {
                println!(
                    "{}",
                    host_text(
                        "journal.cli.host.mcp.activity.request.arguments.exceeded.the.record.bound.and.were.not.stored"
                    )
                );
            }
            Some(request) => {
                // ⚠ Untrusted local strings: rendered as escaped data, never
                // interpreted, and never re-emitted as instructions.
                let rendered = request
                    .arguments
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.activity.request",
                        [("rendered", &(rendered))]
                    )
                );
            }
            None => println!(
                "{}",
                host_text("journal.cli.host.mcp.activity.request.legacy")
            ),
        }
        if let Some(reason) = entry.reason.as_deref() {
            println!(
                "{}",
                host_display(
                    "journal.cli.host.mcp.activity.reason",
                    [("reason", &(reason))]
                )
            );
        }
        if let Some(result) = entry.result.as_ref() {
            let message = if result.targets.is_empty() {
                solstone_core_cli_copy::render_for(
                    host_locale(),
                    "journal.cli.host.mcp.activity.result",
                    &[
                        ("count", CopyValue::String(&(result.count).to_string())),
                        ("digest", CopyValue::String(&(result.digest).to_string())),
                        (
                            "count_quantity",
                            CopyValue::Cardinal(host_count_quantity(result.count as u64)),
                        ),
                    ],
                )
            } else if result.targets_truncated {
                solstone_core_cli_copy::render_for(
                    host_locale(),
                    "journal.cli.host.mcp.activity.result.truncated",
                    &[
                        ("count", CopyValue::String(&(result.count).to_string())),
                        ("digest", CopyValue::String(&(result.digest).to_string())),
                        (
                            "targets",
                            CopyValue::String(&(result.targets.join(", ")).to_string()),
                        ),
                        (
                            "count_quantity",
                            CopyValue::Cardinal(host_count_quantity(result.count as u64)),
                        ),
                    ],
                )
            } else {
                solstone_core_cli_copy::render_for(
                    host_locale(),
                    "journal.cli.host.mcp.activity.result.targets",
                    &[
                        ("count", CopyValue::String(&(result.count).to_string())),
                        ("digest", CopyValue::String(&(result.digest).to_string())),
                        (
                            "targets",
                            CopyValue::String(&(result.targets.join(", ")).to_string()),
                        ),
                        (
                            "count_quantity",
                            CopyValue::Cardinal(host_count_quantity(result.count as u64)),
                        ),
                    ],
                )
            };
            println!("{message}");
            if let Some(origin) = result.origin.as_ref() {
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.activity.origin",
                        [("origin", &(origin))]
                    )
                );
            }
            if let Some(created_at) = result.created_at {
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.activity.created",
                        [("created", &(solstone_core_time::utc_display(created_at)))]
                    )
                );
            }
            if let Some(byte_count) = result.byte_count {
                println!(
                    "{}",
                    host_display(
                        "journal.cli.host.mcp.activity.bytes",
                        [("byte_count", &(byte_count))]
                    )
                );
            }
        }
    }

    // 🔑 A count enters this output with its construction: the population is
    // this page, over this filter, from this bounded walk.
    let counts = tally(&page.entries)
        .into_iter()
        .map(|(outcome, count)| format!("{outcome} {count}"))
        .collect::<Vec<_>>()
        .join(" · ");
    println!();
    let summary = if page.examination_complete {
        solstone_core_cli_copy::render_for(
            host_locale(),
            "journal.cli.host.mcp.activity.shown.complete",
            &[
                (
                    "shown",
                    CopyValue::String(&(page.entries.len()).to_string()),
                ),
                ("examined", CopyValue::String(&(page.examined).to_string())),
                ("counts", CopyValue::String(&(counts).to_string())),
                (
                    "shown_quantity",
                    CopyValue::Cardinal(host_count_quantity(page.entries.len() as u64)),
                ),
            ],
        )
    } else {
        solstone_core_cli_copy::render_for(
            host_locale(),
            "journal.cli.host.mcp.activity.shown.partial",
            &[
                (
                    "shown",
                    CopyValue::String(&(page.entries.len()).to_string()),
                ),
                ("examined", CopyValue::String(&(page.examined).to_string())),
                ("counts", CopyValue::String(&(counts).to_string())),
                (
                    "shown_quantity",
                    CopyValue::Cardinal(host_count_quantity(page.entries.len() as u64)),
                ),
            ],
        )
    };
    println!("{summary}");
    if page.unreadable > 0 {
        println!(
            "{}",
            solstone_core_cli_copy::render_for(
                host_locale(),
                "journal.cli.host.mcp.activity.unreadable",
                &[
                    ("count", CopyValue::String(&(page.unreadable).to_string())),
                    (
                        "count_quantity",
                        CopyValue::Cardinal(host_count_quantity(page.unreadable as u64))
                    )
                ]
            )
        );
    }
    if page.next.is_some() && page.entries.len() >= command.limit {
        println!(
            "{}",
            host_text("journal.cli.host.mcp.activity.raise.limit.or.narrow.day.from.day.to.to.see")
        );
    }
    ExitCode::SUCCESS
}

#[cfg(feature = "journal-mcp-endpoint")]
fn solstone_core_mcp_audit_tool_name(value: &str) -> Option<solstone_core::AuditToolName> {
    solstone_core::AuditToolName::from_token(value)
}

#[cfg(feature = "journal-mcp-endpoint")]
fn run_mcp_probe_command(command: solstone_core_cli::McpProbeCommand) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let key = match resolve_connection_key(&journal.path, &command.target) {
        Ok(key) => key,
        Err(code) => return code,
    };
    let arguments = match serde_json::from_str::<serde_json::Value>(&command.arguments) {
        Ok(value) if value.is_object() => value,
        _ => {
            eprintln!("solstone journal mcp probe: --arguments must be a JSON object");
            return ExitCode::from(EXIT_DATAERR);
        }
    };
    match solstone_core::run_mcp_probe(&journal.path, &key, &command.tool, &arguments) {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(result) => {
                println!("{result}");
                ExitCode::SUCCESS
            }
            Err(_) => ExitCode::from(EXIT_UNAVAILABLE),
        },
        Err(
            solstone_core::McpProbeError::InvalidTool | solstone_core::McpProbeError::InvalidInput,
        ) => {
            eprintln!("solstone journal mcp probe: invalid tool or arguments");
            ExitCode::from(EXIT_DATAERR)
        }
        Err(solstone_core::McpProbeError::PermissionDenied) => {
            eprintln!("solstone journal mcp probe: permission denied");
            ExitCode::from(EXIT_DATAERR)
        }
        Err(solstone_core::McpProbeError::Unavailable) => {
            eprintln!("solstone journal mcp probe: unavailable");
            ExitCode::from(EXIT_UNAVAILABLE)
        }
    }
}

#[cfg(feature = "journal-mcp-endpoint")]
fn resolve_connection_key(
    journal_path: &std::path::Path,
    target: &McpTarget,
) -> Result<String, ExitCode> {
    match target {
        McpTarget::Token { label } => {
            let token_store = solstone_core::TokenStore::open(journal_path);
            match token_store.find_id_by_label(label) {
                Ok(Some(id)) => Ok(format!("bearer:{id}")),
                Ok(None) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.bearer.token.exists.for.label",
                            [("label", &(format!("{:?}", label)))]
                        )
                    );
                    Err(ExitCode::from(EXIT_DATAERR))
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.token.store",
                            [("e", &e.message_for(host_locale()))]
                        )
                    );
                    Err(ExitCode::from(EXIT_TEMPFAIL))
                }
            }
        }
        McpTarget::Oauth {
            client_id,
            created_at,
        } => {
            let oauth_store = solstone_core::OAuthStore::open(journal_path);
            match oauth_store.list_grants() {
                Ok(grants) => {
                    let matches: Vec<_> = grants
                        .into_iter()
                        .filter(|g| g.client_id == *client_id)
                        .filter(|g| {
                            if let Some(created) = created_at {
                                solstone_core_time::utc_rfc3339(g.created_at) == *created
                                    || format!("{}", g.created_at.as_second()) == *created
                            } else {
                                true
                            }
                        })
                        .collect();
                    if matches.is_empty() {
                        eprintln!(
                            "{}",
                            host_display(
                                "journal.cli.host.resolve.connection.key.solstone.journal.mcp.no.oauth.grant.found.matching.client.id",
                                [("client_id", &(format!("{:?}", client_id)))]
                            )
                        );
                        Err(ExitCode::from(EXIT_DATAERR))
                    } else if matches.len() > 1 {
                        eprintln!(
                            "{}",
                            host_display(
                                "journal.cli.host.resolve.connection.key.solstone.journal.mcp.multiple.oauth.grants.matched.client.id.specify",
                                [("client_id", &(format!("{:?}", client_id)))]
                            )
                        );
                        Err(ExitCode::from(EXIT_DATAERR))
                    } else {
                        Ok(format!("oauth:{}", matches[0].id))
                    }
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.resolve.connection.key.solstone.journal.mcp.failed.to.query.oauth.store",
                            [("e", &e.message_for(host_locale()))]
                        )
                    );
                    Err(ExitCode::from(EXIT_TEMPFAIL))
                }
            }
        }
    }
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_permission(_command: McpPermissionCommand) -> ExitCode {
    eprintln!(
        "{}",
        host_text(
            "journal.cli.host.mcp.permission.permission.management.is.not.compiled.into.this.build"
        )
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_activity(_command: solstone_core_cli::McpActivityCommand) -> ExitCode {
    eprintln!(
        "{}",
        host_text("journal.cli.host.mcp.activity.activity.is.not.compiled.into.this.build")
    );
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(not(feature = "journal-mcp-endpoint"))]
fn run_mcp_probe_command(_command: solstone_core_cli::McpProbeCommand) -> ExitCode {
    eprintln!("solstone journal mcp probe is not compiled into this build");
    ExitCode::from(EXIT_UNAVAILABLE)
}

#[cfg(any(unix, windows))]
fn run_spl_service(
    options: ServiceOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.path.clone();
    run_hosted_service(
        &journal.path,
        HostedServiceKind::Spl,
        #[cfg(windows)]
        admitted,
        move |parent| {
            let verbosity =
                solstone_core_spl::Verbosity::from_flags(options.verbose, options.debug);
            match solstone_core_spl::run_native_service_with_hosted_parent(
                service_journal,
                verbosity,
                parent,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.spl.service.spl.service.failed",
                            [("class", &(error.class()))]
                        )
                    );
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        },
    )
}

#[cfg(not(any(unix, windows)))]
fn run_spl_service(
    _options: ServiceOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    unavailable()
}

#[cfg(any(unix, windows))]
fn run_sense_service(
    options: ServiceOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
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
        #[cfg(windows)]
        admitted,
    ) {
        Ok(generation) => generation,
        Err(error) => {
            if let Some(message) =
                solstone_core_transcribe::speakers_installation_detail_in(&error, host_locale())
            {
                eprintln!("{message}");
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    let sense_child_environment = generation.child_launch_context();
    let service_journal = journal.path.clone();
    run_hosted_service(
        &journal.path,
        HostedServiceKind::Sense,
        #[cfg(windows)]
        admitted,
        move |parent| {
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
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.sense.service.sense.service.failed",
                            [("class", &(error.class()))]
                        )
                    );
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        },
    )
}

#[cfg(not(any(unix, windows)))]
fn run_sense_service(
    _options: ServiceOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    unavailable()
}

#[cfg(any(unix, windows))]
fn run_cortex_service(
    options: ServiceOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    let service_journal = journal.path.clone();
    run_hosted_service(
        &journal.path,
        HostedServiceKind::Cortex,
        #[cfg(windows)]
        admitted,
        move |parent| {
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
                    eprintln!(
                        "{}",
                        host_display(
                            "journal.cli.host.cortex.service.cortex.service.failed",
                            [("error", &(error))]
                        )
                    );
                    ExitCode::from(EXIT_TEMPFAIL)
                }
            }
        },
    )
}

#[cfg(not(any(unix, windows)))]
fn run_cortex_service(
    _options: ServiceOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    unavailable()
}

fn run_sense(
    options: SenseOptions,
    #[cfg(windows)] admitted: Option<&solstone_core_system::process::AdmittedWindowsLaunch>,
) -> ExitCode {
    let Some(day) = options.day.clone() else {
        return run_sense_service(
            ServiceOptions {
                verbose: options.verbose,
                debug: options.debug,
            },
            #[cfg(windows)]
            admitted,
        );
    };
    let journal = match resolve_process_journal_path() {
        Ok(journal) => journal,
        Err(error) => {
            eprint_journal_path_error(error);
            return ExitCode::from(EXIT_TEMPFAIL);
        }
    };
    #[cfg(unix)]
    if let Err(error) = acknowledge_hosted_child_admission(&journal.path) {
        eprintln!(
            "{}",
            host_display(
                "journal.cli.host.sense.sense.parent.loss.admission.failed",
                [("error", &(error))]
            )
        );
        return ExitCode::from(EXIT_TEMPFAIL);
    }
    if let Err(error) = solstone_core_transcribe::require_solstone(&journal.path) {
        if let Some(message) =
            solstone_core_transcribe::speakers_installation_detail_in(&error, host_locale())
        {
            eprintln!("{message}");
        }
        return ExitCode::from(error.exit_code() as u8);
    }
    let _generation = match solstone_core_transcribe::enter_speakers_analyze_generation(
        &journal.path,
        solstone_core_transcribe::SpeakersAnalyzeOwnerRole::Sense,
        #[cfg(windows)]
        admitted,
    ) {
        Ok(generation) => generation,
        Err(error) => {
            if let Some(message) =
                solstone_core_transcribe::speakers_installation_detail_in(&error, host_locale())
            {
                eprintln!("{message}");
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    solstone_core_sense::batch::install_batch_signal_handlers();
    let reprocess = options.reprocess.map(|kind| match kind {
        SenseReprocessKind::Screen => solstone_core_sense::batch::ReprocessKind::Screen,
        SenseReprocessKind::Audio => solstone_core_sense::batch::ReprocessKind::Audio,
        SenseReprocessKind::Image => solstone_core_sense::batch::ReprocessKind::Image,
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
        &_generation.child_launch_context(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                host_display(
                    "journal.cli.host.sense.sense.batch.failed",
                    [("error", &(error))]
                )
            );
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

fn run_indexer(command: IndexerCommand) -> ExitCode {
    match command {
        IndexerCommand::Maintenance(options) => run_indexer_maintenance(options),
        IndexerCommand::Search(options) => run_indexer_search(options),
        IndexerCommand::Counts(options) => run_indexer_counts(options),
        IndexerCommand::Agents(options) => run_indexer_agents(options),
        IndexerCommand::Coverage(options) => run_indexer_coverage(options),
        IndexerCommand::PathLookup(options) => run_indexer_path_lookup(options),
        IndexerCommand::Classifications(options) => run_indexer_classifications(options),
        IndexerCommand::PruneStream(options) => run_indexer_prune_stream(options),
        IndexerCommand::PrunePaths(options) => run_indexer_prune_paths(options),
        IndexerCommand::EdgeFingerprint(options) => run_indexer_edge_fingerprint(options),
        IndexerCommand::Drain(options) => run_indexer_drain(options),
    }
}

fn run_indexer_drain(options: IndexerDrainOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let mut drain = solstone_core_indexer_store::todo::DrainOptions {
        force: options.force,
        ..Default::default()
    };
    if let Some(seconds) = options.budget_seconds {
        drain.budget = Duration::from_secs(seconds);
    }
    match solstone_core_indexer_store::todo::drain(&journal, drain) {
        Ok(report) => {
            if options.json {
                print_json(&json!(report));
            } else {
                println!(
                    "applied {} search updates; {} failed; {} remain queued.",
                    report.processed,
                    report.failed,
                    report.remaining.keys.len()
                );
            }
            if report.timed_out || report.failed > 0 {
                ExitCode::from(EXIT_TEMPFAIL)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => print_indexer_mutation_error("drain", error),
    }
}

fn run_indexer_classifications(options: IndexerClassificationsOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let result = if options.drain {
        drain_classifications(&journal)
    } else if options.apply {
        apply_classification_batch(&journal)
    } else {
        inspect_classifications(&journal)
    };
    match result {
        Ok(status) => {
            if options.json {
                print_json(&status.to_json_value());
            } else {
                print!("{}", status.format_human());
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("classifications", error),
    }
}

fn run_indexer_path_lookup(options: IndexerPathLookupOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    let result = if options.apply {
        apply_path_lookup(&journal)
    } else {
        inspect_path_lookup(&journal)
    };
    match result {
        Ok(status) => {
            if options.json {
                print_json(&json!({"ready": status.ready}));
            } else if status.ready {
                println!("path lookup: ready");
            } else {
                println!("path lookup: unready");
            }
            ExitCode::SUCCESS
        }
        Err(error) => print_indexer_mutation_error("path-lookup", error),
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
                    let mut output = json!({
                        "indexed": report.indexed,
                        "removed": report.removed,
                        "skipped": report.skipped,
                        "edges_indexed": report.edges_indexed,
                        "edges_removed": report.edges_removed,
                        "edge_rows_inserted": report.edge_rows_inserted,
                        "merge_steps_spent": report.merge_steps_spent,
                        "failed": report.failed,
                    });
                    if report.deferred_to_full {
                        output["deferred_to_full"] = json!(true);
                    }
                    print_json(&output);
                } else if report.deferred_to_full {
                    print!("{}", solstone_core_indexer_store::scan::FULL_PASS_RUNNING);
                }
                // A full rescan is an operator's repair, so it says when files
                // failed. A light rescan keeps exit 0 for the scheduled callers;
                // its next run retries the file.
                if options.rescan_full && report.failed > 0 {
                    eprintln!(
                        "indexer: {} update(s) couldn't be indexed; see the warnings above",
                        report.failed
                    );
                    return ExitCode::FAILURE;
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

fn run_indexer_edge_fingerprint(options: IndexerReadOptions) -> ExitCode {
    let journal = match resolve_indexer_journal_path(options.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match fingerprint_edge_rows(&journal) {
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
    let mut request = match search_request(
        &options.query,
        options.limit,
        options.offset,
        options.counts,
        &options.order,
    ) {
        Ok(request) => request,
        Err(()) => return print_usage_error(),
    };
    // The owner's own command line reads what the owner wrote on a body record.
    request.body_owner_words = true;
    let journal = match resolve_indexer_journal_path(options.query.journal_override) {
        Ok(line) => line.path,
        Err(error) => return print_journal_error(error),
    };
    match search(&journal, OwnerBoundary, &request, owner_today(&journal)) {
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
    match search_counts(&journal, OwnerBoundary, &request, owner_today(&journal)) {
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
    match agents(&journal, QueryBoundary::Owner) {
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
    match coverage(&journal, QueryBoundary::Owner) {
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
    solstone_core_journal::discover_current_home()
}

// Packaged installs may not retain source-package roots and therefore report zero talent configs.
fn discover_package_roots() -> Option<(PathBuf, PathBuf)> {
    env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf))
        .and_then(|directory| crate::contract::paths::package_roots_from_executable_dir(&directory))
}

fn eprint_journal_path_error(error: JournalPathError) {
    eprint_journal_path_error_in(host_locale(), error);
}

fn eprint_journal_path_error_in(locale: &str, error: JournalPathError) {
    match error {
        JournalPathError::Config(ConfigError::Decode) => {
            eprintln!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.path.utf8", &[])
            )
        }
        JournalPathError::Home(HomeError::Unavailable) => {
            eprintln!(
                "{}",
                solstone_core_cli_copy::render_for(locale, "journal.cli.path.home", &[])
            )
        }
        JournalPathError::Create(error) => eprintln!(
            "{}",
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.path.create",
                [
                    ("source", &error.source),
                    ("path", &error.path.display()),
                    ("error", &error.error)
                ]
            )
        ),
    }
}

/// Today in the journal's owner zone, for relative dates like "yesterday".
fn owner_today(journal: &Path) -> jiff::civil::Date {
    jiff::Timestamp::now()
        .to_zoned_datetime(solstone_core_journal_config::owner_zone(journal))
        .date()
}

#[cfg(test)]
mod tests {

    /// The Windows Task Scheduler worker travels as one `-EncodedCommand`. The
    /// full `powershell.exe` path and flags ride along, so the encoded script
    /// keeps a margin under the command-line limit, and the wire form is only
    /// sound for a script with no here-strings or block comments.
    #[test]
    fn the_windows_task_scheduler_worker_fits_one_command_line() {
        let source = include_str!("service_windows/task_scheduler.ps1");
        for construct in ["@'", "'@", "@\"", "\"@", "<#", "#>"] {
            assert!(
                !source.contains(construct),
                "wire form cannot carry {construct}"
            );
        }
        let wire = solstone_core_service_unit::powershell_wire_script(source);
        let encoded = solstone_core_service_unit::powershell_encoded_length(&wire);
        assert!(
            encoded + 1_024 < solstone_core_service_unit::WINDOWS_COMMAND_LINE_LIMIT,
            "encoded worker is {encoded} characters"
        );
        assert!(wire.contains("$task.Enabled = ($operation -ceq 'enable')"));
        assert!(!wire.lines().any(|line| line.starts_with('#')));
    }
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
        assert_eq!(EXIT_NOINPUT, 66);
        assert_eq!(EXIT_NOHOST, 68);
        assert_eq!(EXIT_UNAVAILABLE, 69);
        assert_eq!(EXIT_CANTCREAT, 73);
        assert_eq!(EXIT_IOERR, 74);
        assert_eq!(EXIT_TEMPFAIL, 75);
        assert_eq!(EXIT_PROTOCOL, 76);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_declared_parent_skips_capture_when_hosting_is_disabled() {
        assert_eq!(resolve_declared_parent(false), Ok(None));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_declared_parent_captures_the_live_direct_parent_when_hosting_is_enabled() {
        assert!(matches!(resolve_declared_parent(true), Ok(Some(_))));
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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
    fn logger_install_is_idempotent_and_defaults_to_warn() {
        install_logger();
        install_logger();
        if std::env::var("RUST_LOG").is_err() {
            assert!(log::max_level() >= log::LevelFilter::Warn);
        }
    }

    #[test]
    fn complete_transcribe_feeds_and_handles_outcomes() {
        let dir_path = PathBuf::from("/var/tmp").join(format!(
            "solstone-complete-transcribe-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir_path);
        fs::create_dir_all(dir_path.join("entities/owner")).unwrap();
        fs::write(
            dir_path.join("entities/owner/entity.json"),
            serde_json::json!({
                "id": "owner",
                "name": "Owner",
                "type": "Person",
                "is_principal": true
            })
            .to_string(),
        )
        .unwrap();

        // Direct segment fixture
        let direct_seg = dir_path.join("chronicle/20260101/093000_300_summary");
        fs::create_dir_all(&direct_seg).unwrap();
        let direct_jsonl = direct_seg.join("audio.jsonl");
        let direct_npz = direct_seg.join("audio.npz");

        let mut unit_vec = vec![0.0_f32; 256];
        unit_vec[0] = 1.0;
        let mut raw_bytes = Vec::new();
        for _ in 0..30 {
            for col in &unit_vec {
                raw_bytes.extend_from_slice(&col.to_le_bytes());
            }
        }
        let payload_path = direct_seg.join("audio_payload.f32");
        fs::write(&payload_path, &raw_bytes).unwrap();

        let sids: Vec<i64> = (1..=30).collect();
        let statements: Vec<serde_json::Value> = sids
            .iter()
            .enumerate()
            .map(|(i, &sid)| {
                serde_json::json!({
                    "id": sid,
                    "sentence_id": sid,
                    "start_offset_us": (i as i64) * 1_000_000,
                    "text": format!("statement {sid}"),
                })
            })
            .collect();

        let req = serde_json::json!({
            "schema": "solstone-speaker-transcript-write-request-v1",
            "output": {
                "jsonl_path": direct_jsonl.display().to_string(),
                "npz_path": direct_npz.display().to_string(),
                "redo": true,
            },
            "base_time_us_of_day": 100_000_u64,
            "source": "audio",
            "statements": statements,
            "header": {
                "raw": "audio.wav",
                "model": "model",
                "device": "cpu",
                "compute_type": "int8",
                "speaker_evidence": "single",
                "speaker_evidence_version": "windowed-slots-v1",
                "speaker_evidence_multi_fraction": 0.0,
            },
            "embeddings": {
                "payload_path": payload_path,
                "payload_format": "raw-f32le-row-major-v1",
                "dtype": "float32-le",
                "shape": [30, 256],
                "byte_count": 30 * 256 * 4,
                "statement_ids": sids,
                "durations_s": vec![1.5; 30],
                "encoder": "test",
            }
        });
        solstone_core_speaker_id::writer::write_request(
            serde_json::to_vec(&req).unwrap().as_slice(),
        )
        .unwrap();

        let direct_file = solstone_core_transcribe::TranscribeFile {
            outcome: solstone_core_transcribe::TranscribeOutcome::Transcribed,
            transcript_path: direct_jsonl.clone(),
            embeddings_path: direct_npz.clone(),
        };
        let run = solstone_core_transcribe::CliRun {
            exit_code: 0,
            summary: Some("1 processed".to_owned()),
            stderr: None,
            files: vec![direct_file.clone()],
        };

        let exit = complete_transcribe(&dir_path, run.clone());
        assert_eq!(exit, ExitCode::SUCCESS);

        let pool_path = dir_path.join("awareness/speaker_candidates.json");
        assert!(pool_path.exists());
        let pool_bytes = fs::read(&pool_path).unwrap();
        let pool_mtime = fs::metadata(&pool_path).unwrap().modified().unwrap();

        let pool_data: serde_json::Value = serde_json::from_slice(&pool_bytes).unwrap();
        let candidates = pool_data["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["n_intervals"].as_u64().unwrap(), 30);
        assert_eq!(candidates[0]["total_duration_s"].as_f64().unwrap(), 45.0);
        let src = &candidates[0]["source_segments"][0];
        assert_eq!(src["stream_layout"].as_str().unwrap(), "direct");
        assert_eq!(src["stream"].as_str().unwrap(), "_default");
        assert_eq!(src["segment_key"].as_str().unwrap(), "093000_300_summary");
        assert_eq!(src["cluster_label"].as_i64().unwrap(), -1);
        assert_eq!(src["sentence_ids"].as_array().unwrap().len(), 30);

        // Second call with same report leaves pool byte-identical
        let exit2 = complete_transcribe(&dir_path, run);
        assert_eq!(exit2, ExitCode::SUCCESS);
        assert_eq!(fs::read(&pool_path).unwrap(), pool_bytes);
        assert_eq!(
            fs::metadata(&pool_path).unwrap().modified().unwrap(),
            pool_mtime
        );

        // Named segment fixture
        let named_seg = dir_path.join("chronicle/20260101/mic/093000_300_summary");
        fs::create_dir_all(&named_seg).unwrap();
        let named_jsonl = named_seg.join("audio.jsonl");
        let named_npz = named_seg.join("audio.npz");
        let named_payload = named_seg.join("audio_payload.f32");
        fs::write(&named_payload, &raw_bytes).unwrap();

        let named_statements: Vec<serde_json::Value> = sids
            .iter()
            .enumerate()
            .map(|(i, &sid)| {
                serde_json::json!({
                    "id": sid,
                    "sentence_id": sid,
                    "speaker": 1,
                    "start_offset_us": (i as i64) * 1_000_000,
                    "text": format!("statement {sid}"),
                })
            })
            .collect();

        let named_req = serde_json::json!({
            "schema": "solstone-speaker-transcript-write-request-v1",
            "output": {
                "jsonl_path": named_jsonl.display().to_string(),
                "npz_path": named_npz.display().to_string(),
                "redo": true,
            },
            "base_time_us_of_day": 100_000_u64,
            "source": "audio",
            "statements": named_statements,
            "header": {
                "raw": "audio.wav",
                "model": "model",
                "device": "cpu",
                "compute_type": "int8",
            },
            "embeddings": {
                "payload_path": named_payload,
                "payload_format": "raw-f32le-row-major-v1",
                "dtype": "float32-le",
                "shape": [30, 256],
                "byte_count": 30 * 256 * 4,
                "statement_ids": sids,
                "durations_s": vec![1.5; 30],
                "encoder": "test",
            }
        });
        solstone_core_speaker_id::writer::write_request(
            serde_json::to_vec(&named_req).unwrap().as_slice(),
        )
        .unwrap();

        let named_file = solstone_core_transcribe::TranscribeFile {
            outcome: solstone_core_transcribe::TranscribeOutcome::Transcribed,
            transcript_path: named_jsonl,
            embeddings_path: named_npz,
        };
        let named_run = solstone_core_transcribe::CliRun {
            exit_code: 0,
            summary: None,
            stderr: None,
            files: vec![named_file],
        };
        complete_transcribe(&dir_path, named_run);

        let pool_data2: serde_json::Value =
            serde_json::from_slice(&fs::read(&pool_path).unwrap()).unwrap();
        let cand = &pool_data2["candidates"].as_array().unwrap()[0];
        let src2 = &cand["source_segments"][1];
        assert_eq!(src2["stream_layout"].as_str().unwrap(), "named");
        assert_eq!(src2["stream"].as_str().unwrap(), "mic");
        assert_eq!(src2["segment_key"].as_str().unwrap(), "093000_300_summary");
        assert_eq!(src2["cluster_label"].as_i64().unwrap(), 1);

        // Non-transcribed / absent npz outcomes leave missing pool missing
        let fresh_dir = PathBuf::from("/var/tmp")
            .join(format!("solstone-complete-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&fresh_dir);
        fs::create_dir_all(fresh_dir.join("entities/owner")).unwrap();
        fs::write(
            fresh_dir.join("entities/owner/entity.json"),
            r#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();

        let absent_run = solstone_core_transcribe::CliRun {
            exit_code: 0,
            summary: None,
            stderr: None,
            files: vec![
                solstone_core_transcribe::TranscribeFile {
                    outcome: solstone_core_transcribe::TranscribeOutcome::Skipped,
                    transcript_path: direct_jsonl.clone(),
                    embeddings_path: direct_npz.clone(),
                },
                solstone_core_transcribe::TranscribeFile {
                    outcome: solstone_core_transcribe::TranscribeOutcome::Preserved,
                    transcript_path: direct_jsonl.clone(),
                    embeddings_path: direct_npz.clone(),
                },
                solstone_core_transcribe::TranscribeFile {
                    outcome: solstone_core_transcribe::TranscribeOutcome::Failed,
                    transcript_path: direct_jsonl.clone(),
                    embeddings_path: direct_npz.clone(),
                },
                solstone_core_transcribe::TranscribeFile {
                    outcome: solstone_core_transcribe::TranscribeOutcome::Transcribed,
                    transcript_path: direct_jsonl.clone(),
                    embeddings_path: fresh_dir.join("nonexistent.npz"),
                },
            ],
        };
        complete_transcribe(&fresh_dir, absent_run);
        assert!(!fresh_dir.join("awareness/speaker_candidates.json").exists());

        // Invalid pool bytes remain unchanged and return matching ExitCode
        fs::create_dir_all(fresh_dir.join("awareness")).unwrap();
        let invalid_bytes = b"not valid json candidate pool";
        let bad_pool = fresh_dir.join("awareness/speaker_candidates.json");
        fs::write(&bad_pool, invalid_bytes).unwrap();

        let err_run = solstone_core_transcribe::CliRun {
            exit_code: 42,
            summary: Some("test summary".to_owned()),
            stderr: Some("test stderr".to_owned()),
            files: vec![direct_file],
        };
        let err_exit = complete_transcribe(&fresh_dir, err_run);
        assert_eq!(err_exit, ExitCode::from(42));
        assert_eq!(fs::read(&bad_pool).unwrap(), invalid_bytes);

        let _ = fs::remove_dir_all(dir_path);
        let _ = fs::remove_dir_all(fresh_dir);
    }

    #[cfg(all(test, feature = "full-tests"))]
    include!("tests/indexer_path_lookup.rs");

    #[cfg(all(test, feature = "full-tests"))]
    include!("tests/indexer_classifications.rs");
}

#[cfg(all(test, unix, feature = "journal-mcp-endpoint"))]
mod mcp_pairing_cli_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn read_active_pairing(journal_root: &Path) -> Option<serde_json::Value> {
        let path = journal_root.join("mcp-endpoint/oauth.json");
        if !path.exists() {
            return None;
        }
        let data = fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&data).unwrap();
        val.get("pairing")
            .and_then(|p| p.as_object())
            .cloned()
            .map(serde_json::Value::Object)
    }

    #[test]
    fn run_mcp_pairing_at_mint_default() {
        let dir = tempdir().unwrap();
        let code = run_mcp_pairing_at(dir.path(), McpPairingCommand::Generate { door: None });
        assert_eq!(code, ExitCode::SUCCESS);
        let pairing = read_active_pairing(dir.path()).unwrap();
        assert_eq!(pairing.get("door").and_then(|d| d.as_str()), Some("local"));
        assert_eq!(pairing.get("config_generation"), None);
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_local() {
        let dir = tempdir().unwrap();
        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("local".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        let pairing = read_active_pairing(dir.path()).unwrap();
        assert_eq!(pairing.get("door").and_then(|d| d.as_str()), Some("local"));
        assert_eq!(pairing.get("config_generation"), None);
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_lan() {
        let dir = tempdir().unwrap();
        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("lan".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        let pairing = read_active_pairing(dir.path()).unwrap();
        assert_eq!(
            pairing.get("door").and_then(|d| d.as_str()),
            Some(solstone_core_journal_config::MCP_LAN_DOOR_RESOURCE)
        );
        assert_eq!(pairing.get("config_generation"), None);
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_solstone_me() {
        let dir = tempdir().unwrap();
        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("solstone.me".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        let pairing = read_active_pairing(dir.path()).unwrap();
        assert_eq!(pairing.get("door").and_then(|d| d.as_str()), Some("relay"));
        assert_eq!(pairing.get("config_generation"), None);
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_hostname_valid() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/journal.json"),
            r#"{
  "mcp_endpoint": {
    "byo_hostname": {
      "hostname": "my-host.example.com",
      "enabled": true,
      "generation": 42
    }
  }
}"#,
        )
        .unwrap();
        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("hostname".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::SUCCESS);
        let pairing = read_active_pairing(dir.path()).unwrap();
        assert_eq!(
            pairing.get("door").and_then(|d| d.as_str()),
            Some("https://my-host.example.com/mcp")
        );
        assert_eq!(
            pairing.get("config_generation").and_then(|g| g.as_u64()),
            Some(42)
        );
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_hostname_unconfigured() {
        let dir = tempdir().unwrap();
        let oauth_path = dir.path().join("mcp-endpoint/oauth.json");
        let mint_code = run_mcp_pairing_at(dir.path(), McpPairingCommand::Generate { door: None });
        assert_eq!(mint_code, ExitCode::SUCCESS);
        let before_bytes = fs::read(&oauth_path).unwrap();

        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("hostname".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::from(EXIT_NOHOST));
        assert_eq!(fs::read(&oauth_path).unwrap(), before_bytes);
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_hostname_corrupt_config() {
        let dir = tempdir().unwrap();
        let oauth_path = dir.path().join("mcp-endpoint/oauth.json");
        let mint_code = run_mcp_pairing_at(dir.path(), McpPairingCommand::Generate { door: None });
        assert_eq!(mint_code, ExitCode::SUCCESS);
        let before_bytes = fs::read(&oauth_path).unwrap();

        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("config/journal.json"), "invalid json {[[[").unwrap();

        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("hostname".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::from(EXIT_NOINPUT));
        assert_eq!(fs::read(&oauth_path).unwrap(), before_bytes);
    }

    #[test]
    fn run_mcp_pairing_at_mint_door_hostname_invalid_config() {
        let dir = tempdir().unwrap();
        let oauth_path = dir.path().join("mcp-endpoint/oauth.json");
        let mint_code = run_mcp_pairing_at(dir.path(), McpPairingCommand::Generate { door: None });
        assert_eq!(mint_code, ExitCode::SUCCESS);
        let before_bytes = fs::read(&oauth_path).unwrap();

        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(
            dir.path().join("config/journal.json"),
            r#"{"mcp_endpoint":{"byo_hostname":{"hostname":"ok.example","enabled":"yes","generation":1}}}"#,
        )
        .unwrap();

        let code = run_mcp_pairing_at(
            dir.path(),
            McpPairingCommand::Generate {
                door: Some("hostname".to_owned()),
            },
        );
        assert_eq!(code, ExitCode::from(EXIT_NOINPUT));
        assert_eq!(fs::read(&oauth_path).unwrap(), before_bytes);
    }
}
