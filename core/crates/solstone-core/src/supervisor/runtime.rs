// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_cli::SupervisorOptions;
use solstone_core_journal_config::read_direct_door_port;
#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
use solstone_core_journal_config::{
    McpEndpointCapability, mcp_endpoint_capability, read_journal_config,
};
use solstone_core_journal_config_write::persist_direct_door_port;
use solstone_core_journal_io::{JsonWriteOptions, write_json};
use solstone_core_local::plan::Platform;
use solstone_core_system::cap::{DEFAULT_TASK_MAX_RUNTIME, DefaultCapResolver};
use solstone_core_system::direct_door::{
    initialize_direct_door, peek_direct_door_generation, withhold_direct_door,
};
use solstone_core_system::lifecycle::{
    DEFAULT_INTERVAL_SECONDS, HostedServiceKind, ParentLossCoordinator, ParentLossLedger,
    ParentLossPhase, ParentLossReason, ParentLossTerminalDisposition, ParentWatch,
    ParentWatchStatus, PreReadySupervisorLifecycle, ShutdownRegime, SupervisorLifecycle,
    SyncPeerObservation, SyncSnapshot, SyncTickOutcome, write_retire_expected_control,
};
use solstone_core_system::process::{
    CommandLaunchRequest, Disposition, HostedLaunchProvenance, InspectResult, InstanceVerdict,
    LaunchError, LaunchedProcessIdentity, ManagedLaunchRequest, ManagedProcess, ProcessInstance,
    ProcessInstanceSource, RestartPolicy, STRUGGLING_THRESHOLD, SpawnOptions,
    SystemProcessInstanceSource, describe_exit, launch_command, launch_managed_hosted,
};
use solstone_core_system::provider_runtime::{
    FileRuntimeStore, LocalLifecycleSeam, LocalProbeSeam, LocalRuntimeShared, LocalTruthConfig,
    LocalTruthSeam, ParakeetLifecycleSeam, ParakeetProbeSeam, ParakeetRuntimeShared,
    ParakeetTruthConfig, ParakeetTruthSeam, ProviderName, ProviderRuntimeCoordinator,
    ProviderRuntimeState, ReasonCode, RuntimePhase, SystemRuntimeClock, WedgeState,
};
use solstone_core_system::queue::{SystemProcessStateProbe, TaskQueue, TaskQueueOptions};
use solstone_core_system::schedule::{ScheduleEngine, ScheduleNow, initialize_schedule_config};
use solstone_core_system::status_wire::CrashedServiceCandidate;

use super::bus::{SupervisorProcessSink, SupervisorScheduleSink, SupervisorTaskQueueSink};
use super::shutdown::{StderrBoundedShutdownDiagnosticSink, SupervisorShutdownDriver};
use super::tick;

const APP_FIXTURE_ENABLED_ENV: &str = "SOLSTONE_SUPERVISOR_APP_FIXTURE";
const APP_FIXTURE_BINARY_ENV: &str = "SOLSTONE_SUPERVISOR_APP_BINARY";
/// Enables short scheduling intervals for the app-process integration fixture.
/// This is only honored while the fixture binary itself is enabled.
const APP_FIXTURE_FAST_TIMING_ENV: &str = "SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING";
const PARAKEET_FIXTURE_ENV: &str = "SOLSTONE_SUPERVISOR_PARAKEET_FIXTURE";
/// Fixture Convey argv override; test-constructed paths must not contain spaces.
const APP_FIXTURE_CONVEY_ARGV_ENV: &str = "SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV";
/// When set, Sense's fixture argv is the real speakers-analyze generation
/// holder binary instead of the plain `ready-park` fixture, so a
/// process-tree test can prove an actual inherited descriptor capability.
const SENSE_GENERATION_HOLDER_ENV: &str = "SOLSTONE_SUPERVISOR_SENSE_GENERATION_HOLDER";
const CONVEY_READY_WINDOW: Duration = Duration::from_secs(60);
const CONVEY_READY_INTERVAL: Duration = Duration::from_millis(100);
const CONVEY_READY_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
const FIXTURE_CONVEY_READY_WINDOW: Duration = Duration::from_secs(3);
const FIXTURE_CONVEY_READY_INTERVAL: Duration = Duration::from_millis(20);
const FAST_FIXTURE_CONVEY_READY_WINDOW: Duration = Duration::from_secs(1);
const FAST_FIXTURE_CONVEY_READY_INTERVAL: Duration = Duration::from_millis(5);
const FAST_FIXTURE_PRE_READY_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(20);
const CALLOSUM_CONNECTION_READY_WINDOW: Duration = Duration::from_secs(2);
const CALLOSUM_CONNECTION_READY_INTERVAL: Duration = Duration::from_millis(5);
const PARENT_LOSS_COORDINATOR_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(3);
const PARENT_LOSS_CHILD_ADMISSION_TIMEOUT: Duration = Duration::from_secs(3);

/// The typed refusal returned when hosted start cannot establish the sole
/// parent-loss terminal authority before it could admit any service work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentLossCoordinatorBootstrapFailure {
    Launch,
    IdentityEstablishment,
    InitialAdmissionHandshake,
    CoordinatorRetirementUnverified,
}

impl std::fmt::Display for ParentLossCoordinatorBootstrapFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Launch => formatter.write_str("coordinator launch failed"),
            Self::IdentityEstablishment => {
                formatter.write_str("coordinator exact identity establishment failed")
            }
            Self::InitialAdmissionHandshake => {
                formatter.write_str("coordinator initial-admission handshake failed")
            }
            Self::CoordinatorRetirementUnverified => {
                formatter.write_str("coordinator exact retirement could not be verified")
            }
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub(crate) enum ParentLossCoordinatorBootstrapTestFault {
    Launch,
    IdentityEstablishment,
    InitialAdmissionHandshake,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_FAULT: Cell<Option<ParentLossCoordinatorBootstrapTestFault>> = const { Cell::new(None) };
    static PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_SPAWNED: RefCell<Option<ProcessInstance>> = const { RefCell::new(None) };
}

#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub(crate) fn set_parent_loss_coordinator_bootstrap_test_fault(
    fault: Option<ParentLossCoordinatorBootstrapTestFault>,
) {
    PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_FAULT.with(|slot| slot.set(fault));
    PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_SPAWNED.with(|slot| *slot.borrow_mut() = None);
}

#[cfg(any(test, feature = "test-hooks"))]
fn parent_loss_coordinator_bootstrap_test_fault() -> Option<ParentLossCoordinatorBootstrapTestFault>
{
    PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_FAULT.with(Cell::get)
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_parent_loss_coordinator_bootstrap_test_spawn(instance: ProcessInstance) {
    PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_SPAWNED.with(|slot| *slot.borrow_mut() = Some(instance));
}

#[cfg(test)]
fn parent_loss_coordinator_bootstrap_test_spawned() -> Option<ProcessInstance> {
    PARENT_LOSS_COORDINATOR_BOOTSTRAP_TEST_SPAWNED.with(|slot| *slot.borrow())
}

pub(crate) struct SupervisorTiming {
    pub tick_interval: Duration,
    pub status_interval: Duration,
}

impl SupervisorTiming {
    fn for_app_fixture(fast: bool) -> Self {
        if fast {
            return Self {
                tick_interval: Duration::from_millis(10),
                status_interval: Duration::from_millis(50),
            };
        }
        Self {
            tick_interval: Duration::from_secs(1),
            status_interval: Duration::from_secs(5),
        }
    }
}

pub(crate) struct SupervisorState {
    pub journal: PathBuf,
    pub is_remote_mode: bool,
    pub no_daily: bool,
    pub server: Arc<CallosumSocketServer>,
    pub connection: CallosumSocketConnection,
    pub queue: TaskQueue,
    pub last_sync_snapshot: Option<SyncSnapshot>,
    pub stale_heartbeats: Vec<SyncPeerObservation>,
    pub shutdown_started: AtomicBool,
    pub started: Instant,
    pub scheduler: Option<ScheduleEngine>,
    pub recorded_schedule_completions: BTreeSet<String>,
    pub app_processes: Vec<ManagedAppProcess>,
    pub local: LocalProvider,
    pub parakeet: ParakeetProvider,
    pub flush: FlushState,
    pub daily: DailyState,
    /// Last completed retry-expiry scan; kept per supervisor instance so a
    /// restart cannot inherit another instance's throttle state.
    pub last_retry_expiry_drain: Instant,
    pub wedge: WedgeState,
    pub timing: SupervisorTiming,
    pub parent_loss_coordinator: Option<ParentLossCoordinatorSession>,
    /// Inherited speakers-analyze installation generation (see
    /// `solstone-core-transcribe::SpeakersAnalyzeGeneration`), merged into
    /// `AppService::Sense`'s spawn environment and into scheduled catchup
    /// think tasks so native transcribe children borrow this supervisor's
    /// generation instead of each attempting their own acquisition.
    pub sense_child_environment: BTreeMap<OsString, OsString>,
    /// Windows Parakeet credentials rotate for every provider launch. This is
    /// the revision already inherited by the current Sense process tree.
    #[cfg(windows)]
    pub parakeet_sense_credentials_revision: u64,
}

/// Supervisor-held capability for the independent coordinator. The random
/// bytes are sent once over the coordinator stdin and otherwise remain only in
/// the two process memories; they authenticate graceful-retirement control.
pub(crate) struct ParentLossCoordinatorSession {
    generation: u64,
    supervisor: ProcessInstance,
    capability: Vec<u8>,
}

impl ParentLossCoordinatorSession {
    pub(crate) fn write_retire_expected(&self, journal: &Path) -> Result<(), String> {
        write_retire_expected_control(journal, self.generation, self.supervisor, &self.capability)
            .map_err(|error| error.to_string())
    }

    /// Wait only for the coordinator's durable graceful-retirement
    /// acknowledgement. A timeout deliberately does not alter lifecycle state:
    /// the supervisor must still finish its own shutdown and the coordinator
    /// remains the sole terminal authority.
    pub(crate) fn wait_for_retire_expected_ack(
        &self,
        journal: &Path,
        timeout: Duration,
    ) -> Result<bool, String> {
        let ledger = ParentLossLedger::open(journal).map_err(|error| error.to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            let Some(active) = ledger
                .active_generation()
                .map_err(|error| error.to_string())?
            else {
                return Ok(false);
            };
            if active.generation != self.generation || active.supervisor != self.supervisor {
                return Ok(false);
            }
            if active.phase == ParentLossPhase::RetiringAcknowledged {
                return Ok(true);
            }
            if active.phase == ParentLossPhase::Terminal {
                return Ok(matches!(
                    ledger.record(self.generation).map_err(|error| error.to_string())?,
                    Some(record) if matches!(record.terminal, Some(ParentLossTerminalDisposition::RetiredExpected))
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }
}

#[derive(Default)]
pub(crate) struct FlushState {
    pub last_segment_ts: Option<Instant>,
    pub day: Option<String>,
    pub segment: Option<String>,
    pub stream: Option<String>,
    pub flushed: bool,
}

pub(crate) struct DailyState {
    pub last_day: Option<chrono::NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppService {
    Convey,
    Sense,
    Cortex,
    Spl,
    #[cfg_attr(not(all(unix, feature = "journal-mcp-endpoint")), allow(dead_code))]
    Mcp,
}

impl AppService {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Convey => "convey",
            Self::Sense => "sense",
            Self::Cortex => "cortex",
            Self::Spl => "spl",
            Self::Mcp => "mcp",
        }
    }

    const fn hosted_service_kind(self) -> HostedServiceKind {
        match self {
            Self::Convey => HostedServiceKind::Convey,
            Self::Sense => HostedServiceKind::Sense,
            Self::Cortex => HostedServiceKind::Cortex,
            Self::Spl => HostedServiceKind::Spl,
            Self::Mcp => HostedServiceKind::Mcp,
        }
    }

    fn production_argv(self, journal_binary: &Path, convey_port: u16) -> Vec<String> {
        let mut argv = vec![journal_binary.display().to_string()];
        match self {
            Self::Convey => argv.extend([
                "convey".to_owned(),
                "--port".to_owned(),
                convey_port.to_string(),
            ]),
            Self::Sense => argv.push("sense".to_owned()),
            Self::Cortex => argv.push("cortex".to_owned()),
            Self::Spl => argv.push("spl".to_owned()),
            Self::Mcp => argv.extend(["mcp".to_owned(), "service".to_owned()]),
        }
        argv
    }
}

fn current_supervisor_instance() -> Result<ProcessInstance, String> {
    match SystemProcessInstanceSource.inspect(std::process::id()) {
        InspectResult::Present { instance, .. } => Ok(instance),
        InspectResult::Absent | InspectResult::Unverifiable => Err(
            "could not verify supervisor process identity for parent-loss coordination".to_owned(),
        ),
    }
}

async fn bootstrap_parent_loss_coordinator(
    journal: &Path,
    supervisor: ProcessInstance,
    enabled: Vec<HostedServiceKind>,
    supervisor_heartbeat_filename: String,
) -> Result<ParentLossCoordinatorSession, ParentLossCoordinatorBootstrapFailure> {
    #[cfg(any(test, feature = "test-hooks"))]
    if parent_loss_coordinator_bootstrap_test_fault()
        == Some(ParentLossCoordinatorBootstrapTestFault::Launch)
    {
        return Err(ParentLossCoordinatorBootstrapFailure::Launch);
    }
    let mut capability = vec![0_u8; 32];
    getrandom::fill(&mut capability)
        .map_err(|_| ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake)?;
    let mut authority = launch_command(
        Disposition::ExplicitlyUnowned {
            reason: "parent-loss coordinator must observe supervisor exit".to_owned(),
        },
        parent_loss_coordinator_launch_request(
            journal,
            supervisor,
            &enabled,
            &supervisor_heartbeat_filename,
        )?,
        Box::new(|child, _| child.kill().map_err(LaunchError::Terminate)),
    )
    .map_err(|_| ParentLossCoordinatorBootstrapFailure::Launch)?;
    let coordinator = match SystemProcessInstanceSource.inspect(authority.pid()) {
        InspectResult::Present { instance, uid, .. } => LaunchedProcessIdentity { instance, uid },
        InspectResult::Absent | InspectResult::Unverifiable => {
            if authority.terminate(Duration::from_secs(2)).is_err()
                || !matches!(authority.poll(), Ok(Some(_)))
            {
                return Err(ParentLossCoordinatorBootstrapFailure::CoordinatorRetirementUnverified);
            }
            return Err(ParentLossCoordinatorBootstrapFailure::IdentityEstablishment);
        }
    };
    authority
        .bind_exact_identity(coordinator)
        .map_err(|_| ParentLossCoordinatorBootstrapFailure::IdentityEstablishment)?;
    #[cfg(any(test, feature = "test-hooks"))]
    record_parent_loss_coordinator_bootstrap_test_spawn(coordinator.instance);
    #[cfg(any(test, feature = "test-hooks"))]
    if parent_loss_coordinator_bootstrap_test_fault()
        == Some(ParentLossCoordinatorBootstrapTestFault::IdentityEstablishment)
    {
        retire_bootstrap_coordinator(&mut authority, coordinator)?;
        return Err(ParentLossCoordinatorBootstrapFailure::IdentityEstablishment);
    }
    let Some(mut stdin) = authority.take_stdin() else {
        retire_bootstrap_coordinator(&mut authority, coordinator)?;
        return Err(ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake);
    };
    if stdin
        .write_all(&capability)
        .and_then(|()| stdin.flush())
        .is_err()
    {
        drop(stdin);
        retire_bootstrap_coordinator(&mut authority, coordinator)?;
        return Err(ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake);
    }
    drop(stdin);
    #[cfg(any(test, feature = "test-hooks"))]
    if parent_loss_coordinator_bootstrap_test_fault()
        == Some(ParentLossCoordinatorBootstrapTestFault::InitialAdmissionHandshake)
    {
        retire_bootstrap_coordinator(&mut authority, coordinator)?;
        return Err(ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake);
    }
    let deadline = Instant::now() + PARENT_LOSS_COORDINATOR_BOOTSTRAP_TIMEOUT;
    while Instant::now() < deadline {
        match ParentLossCoordinator::read_bootstrap_ready(journal) {
            Ok(Some(ready))
                if ready.coordinator == coordinator.instance
                    && ParentLossCoordinator::bootstrap_ready_is_authenticated(
                        &ready,
                        &capability,
                    ) =>
            {
                authority.relinquish_explicitly_unowned().map_err(|error| {
                    let _ = error;
                    ParentLossCoordinatorBootstrapFailure::CoordinatorRetirementUnverified
                })?;
                return Ok(ParentLossCoordinatorSession {
                    generation: ready.generation,
                    supervisor,
                    capability,
                });
            }
            Ok(_) => {}
            Err(error) => {
                let _ = error;
                retire_bootstrap_coordinator(&mut authority, coordinator)?;
                return Err(ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake);
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    retire_bootstrap_coordinator(&mut authority, coordinator)?;
    Err(ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake)
}

fn parent_loss_coordinator_launch_request(
    journal: &Path,
    supervisor: ProcessInstance,
    enabled: &[HostedServiceKind],
    supervisor_heartbeat_filename: &str,
) -> Result<CommandLaunchRequest, ParentLossCoordinatorBootstrapFailure> {
    #[cfg(any(test, feature = "test-hooks"))]
    if parent_loss_coordinator_bootstrap_test_fault().is_some() {
        return Ok(CommandLaunchRequest {
            program: OsString::from("/bin/sleep"),
            arguments: vec![OsString::from("60")],
            environment: BTreeMap::new(),
            current_dir: None,
            process_group: true,
            stdin_piped: true,
            stdout_piped: false,
            stderr_piped: false,
        });
    }
    let executable =
        std::env::current_exe().map_err(|_| ParentLossCoordinatorBootstrapFailure::Launch)?;
    let supervisor_json = serde_json::to_string(&supervisor)
        .map_err(|_| ParentLossCoordinatorBootstrapFailure::Launch)?;
    let enabled_json = serde_json::to_string(enabled)
        .map_err(|_| ParentLossCoordinatorBootstrapFailure::Launch)?;
    Ok(CommandLaunchRequest {
        program: executable.into_os_string(),
        arguments: vec![
            OsString::from("__parent-loss-coordinator"),
            OsString::from("--supervisor-json"),
            OsString::from(supervisor_json),
            OsString::from("--enabled-json"),
            OsString::from(enabled_json),
            OsString::from("--supervisor-heartbeat"),
            OsString::from(supervisor_heartbeat_filename),
        ],
        environment: BTreeMap::from([(
            OsString::from("SOLSTONE_JOURNAL"),
            journal.as_os_str().to_os_string(),
        )]),
        current_dir: None,
        // The coordinator's sole role after admission is to observe the
        // supervisor and publish its terminal ledger result. It must not
        // share the supervisor's containment group: a parent-loss cleanup may
        // retire that group before the coordinator has observed the death.
        process_group: true,
        stdin_piped: true,
        stdout_piped: false,
        stderr_piped: false,
    })
}

fn retire_bootstrap_coordinator(
    authority: &mut solstone_core_system::process::LaunchAuthority,
    coordinator: LaunchedProcessIdentity,
) -> Result<(), ParentLossCoordinatorBootstrapFailure> {
    authority
        .terminate_exact(Duration::from_secs(2))
        .map_err(|_| ParentLossCoordinatorBootstrapFailure::CoordinatorRetirementUnverified)?;
    match SystemProcessInstanceSource.observe(&coordinator.instance) {
        InstanceVerdict::NotSameOrExited => Ok(()),
        InstanceVerdict::SameLive { .. } | InstanceVerdict::Unverifiable => {
            Err(ParentLossCoordinatorBootstrapFailure::CoordinatorRetirementUnverified)
        }
    }
}

pub(crate) struct BackoffState {
    pub reason: String,
    pub exit_code: Option<i32>,
    pub restart_attempts: u32,
}

pub(crate) enum AppExit {
    Process { code: i32 },
    SpawnFailure,
}

#[derive(Serialize)]
struct BackoffRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    restart_attempts: u32,
    reason: String,
}

pub(crate) struct ManagedAppProcess {
    pub service: AppService,
    pub enabled: bool,
    pub argv: Vec<String>,
    pub process: Option<ManagedProcess>,
    pub started_at: Option<Instant>,
    pub restart_policy: RestartPolicy,
    pub restart_at: Option<Instant>,
    fast_fixture_timing: bool,
    pub backoff: Option<BackoffState>,
    /// Generation claimed from `health/direct-door.json` when this Convey child spawned.
    pub direct_door_generation: Option<u64>,
}

impl ManagedAppProcess {
    pub(super) fn new(
        service: AppService,
        enabled: bool,
        journal: &Path,
        fixture_binary: Option<&str>,
        journal_binary: Option<&Path>,
        convey_port: u16,
        fast_fixture_timing: bool,
    ) -> Self {
        let argv = fixture_binary.map_or_else(
            || {
                journal_binary.map_or_else(Vec::new, |binary| {
                    service.production_argv(binary, convey_port)
                })
            },
            |binary| fixture_argv(service, binary, journal),
        );
        Self {
            service,
            enabled,
            argv,
            process: None,
            started_at: None,
            restart_policy: RestartPolicy::default(),
            restart_at: None,
            fast_fixture_timing,
            backoff: None,
            direct_door_generation: None,
        }
    }

    pub(crate) fn record_exit(&mut self, exit: AppExit) -> Duration {
        let uptime = self
            .started_at
            .take()
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        self.process = None;
        let (policy_code, reason, exit_code) = match exit {
            AppExit::Process { code } => (code, describe_exit(code), Some(code)),
            AppExit::SpawnFailure => (-1, "failed to spawn process".to_owned(), None),
        };
        let delay = self.restart_policy.decide_after_exit(policy_code, uptime);
        self.restart_at = Some(
            Instant::now()
                + if self.fast_fixture_timing {
                    delay.min(Duration::from_millis(10))
                } else {
                    delay
                },
        );
        self.backoff =
            (self.restart_policy.unsuccessful_starts() >= STRUGGLING_THRESHOLD).then(|| {
                BackoffState {
                    reason,
                    exit_code,
                    restart_attempts: u32::try_from(self.restart_policy.unsuccessful_starts())
                        .unwrap_or(u32::MAX),
                }
            });
        delay
    }

    pub(crate) fn crashed_candidate(&self) -> Option<CrashedServiceCandidate> {
        let backoff = self.backoff.as_ref()?;
        Some(CrashedServiceCandidate {
            name: self.service.as_str().to_owned(),
            restart_attempts: backoff.restart_attempts,
            phase: RuntimePhase::Backoff,
            reason_code: Some(ReasonCode::from_wire(backoff.reason.clone())),
        })
    }
}

fn backoff_path(journal: &Path, service: AppService) -> PathBuf {
    journal
        .join("health")
        .join(format!("{}.backoff", service.as_str()))
}

fn write_backoff_record(journal: &Path, app: &ManagedAppProcess) {
    let Some(backoff) = app.backoff.as_ref() else {
        return;
    };
    if let Err(error) = write_json(
        backoff_path(journal, app.service),
        &BackoffRecord {
            exit_code: backoff.exit_code,
            restart_attempts: backoff.restart_attempts,
            reason: backoff.reason.clone(),
        },
        JsonWriteOptions::default(),
    ) {
        eprintln!(
            "supervisor: failed to write {}.backoff: {error}",
            app.service.as_str()
        );
    }
}

fn clear_backoff_record(journal: &Path, service: AppService) {
    let _ = std::fs::remove_file(backoff_path(journal, service));
}

fn selected_direct_door_port(journal: &Path, requested: Option<u16>) -> Result<u16, String> {
    requested
        .map(Ok)
        .unwrap_or_else(|| read_direct_door_port(journal).map_err(|error| error.to_string()))
}

pub(crate) fn apply_app_exit(app: &mut ManagedAppProcess, journal: &Path, exit: AppExit) {
    if let Err(error) = withhold_app_direct_door(app, journal) {
        eprintln!("supervisor: failed to withhold direct-door record: {error}");
    }
    app.record_exit(exit);
    if app.backoff.is_some() {
        write_backoff_record(journal, app);
    } else {
        clear_backoff_record(journal, app.service);
    }
}

fn withhold_app_direct_door(app: &mut ManagedAppProcess, journal: &Path) -> Result<(), String> {
    if app.service != AppService::Convey {
        return Ok(());
    }
    let Some(generation) = app.direct_door_generation else {
        return Ok(());
    };
    let port = read_direct_door_port(journal)
        .map_err(|error| format!("failed to read direct-door port: {error}"))?;
    withhold_direct_door(journal, generation, port)
        .map_err(|error| format!("generation {generation} could not be withheld: {error}"))?;
    app.direct_door_generation = None;
    Ok(())
}

fn fixture_marker_path(journal: &Path, service: AppService) -> String {
    journal
        .join("health")
        .join(format!("fixture-{}.marker", service.as_str()))
        .display()
        .to_string()
}

fn fixture_argv(service: AppService, binary: &str, journal: &Path) -> Vec<String> {
    if service == AppService::Sense
        && let Ok(holder) = std::env::var(SENSE_GENERATION_HOLDER_ENV)
    {
        return vec![
            holder,
            journal.to_string_lossy().into_owned(),
            fixture_marker_path(journal, service),
        ];
    }
    let mut argv = vec![binary.to_owned()];
    if service == AppService::Convey {
        if let Ok(override_argv) = std::env::var(APP_FIXTURE_CONVEY_ARGV_ENV) {
            argv.extend(override_argv.split_whitespace().map(str::to_owned));
            return argv;
        }
        argv.extend([
            "ready-sleep".to_owned(),
            fixture_marker_path(journal, service),
            "5000".to_owned(),
        ]);
        return argv;
    }
    argv.extend([
        "ready-park".to_owned(),
        fixture_marker_path(journal, service),
    ]);
    argv
}

fn ready_sleep_marker_path(argv: &[String]) -> Option<&str> {
    match argv.get(1).map(String::as_str) {
        Some("ready-sleep") | Some("ready-sleep-crash-once") => argv.get(2).map(String::as_str),
        _ => None,
    }
}

fn resolve_journal_binary_from(exe_dir: &Path) -> PathBuf {
    exe_dir.join("solstone-core-journal")
}

fn resolve_journal_binary() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let exe_dir = executable.parent().ok_or_else(|| {
        format!(
            "supervisor executable has no parent: {}",
            executable.display()
        )
    })?;
    // The journal shim only delegates to this sibling binary, so direct execution
    // is equivalent, removes an exec hop, and does not depend on PATH.
    Ok(resolve_journal_binary_from(exe_dir))
}

#[derive(Debug)]
pub(crate) enum JournalBinaryPreflightError {
    CurrentExecutable,
    MissingOrNotExecutable { path: PathBuf },
    InvalidLayout,
}

pub(crate) fn preflight_journal_binary(
    options: &SupervisorOptions,
) -> Result<Option<PathBuf>, JournalBinaryPreflightError> {
    if app_fixture_binary().is_some()
        || options.remote.as_deref().is_some_and(|url| !url.is_empty())
    {
        return Ok(None);
    }
    let path =
        resolve_journal_binary().map_err(|_| JournalBinaryPreflightError::CurrentExecutable)?;
    validate_journal_binary(path).map(Some)
}

fn validate_journal_binary(path: PathBuf) -> Result<PathBuf, JournalBinaryPreflightError> {
    let metadata = std::fs::metadata(&path)
        .map_err(|_| JournalBinaryPreflightError::MissingOrNotExecutable { path: path.clone() })?;
    if !metadata.is_file() {
        return Err(JournalBinaryPreflightError::InvalidLayout);
    }
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 == 0 {
        return Err(JournalBinaryPreflightError::MissingOrNotExecutable { path });
    }
    Ok(path)
}

fn resolve_available_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

trait ConveyReadinessProbe: Send + Sync {
    fn is_ready(&self, journal: &Path, convey_argv: &[String]) -> bool;
    fn wait_window(&self) -> Duration;
    fn poll_interval(&self) -> Duration;
}

struct TcpConveyReadinessProbe;

impl ConveyReadinessProbe for TcpConveyReadinessProbe {
    fn is_ready(&self, journal: &Path, _: &[String]) -> bool {
        let Some(port) = std::fs::read_to_string(journal.join("health/convey.port"))
            .ok()
            .and_then(|text| text.trim().parse::<u16>().ok())
        else {
            return false;
        };
        TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            CONVEY_READY_CONNECT_TIMEOUT,
        )
        .is_ok()
    }

    fn wait_window(&self) -> Duration {
        CONVEY_READY_WINDOW
    }

    fn poll_interval(&self) -> Duration {
        CONVEY_READY_INTERVAL
    }
}

struct FixtureConveyReadinessProbe {
    fast_timing: bool,
}

impl ConveyReadinessProbe for FixtureConveyReadinessProbe {
    fn is_ready(&self, _: &Path, convey_argv: &[String]) -> bool {
        ready_sleep_marker_path(convey_argv).is_some_and(|path| Path::new(path).exists())
    }

    fn wait_window(&self) -> Duration {
        if self.fast_timing {
            FAST_FIXTURE_CONVEY_READY_WINDOW
        } else {
            FIXTURE_CONVEY_READY_WINDOW
        }
    }

    fn poll_interval(&self) -> Duration {
        if self.fast_timing {
            FAST_FIXTURE_CONVEY_READY_INTERVAL
        } else {
            FIXTURE_CONVEY_READY_INTERVAL
        }
    }
}

pub(crate) struct LocalProvider {
    pub coordinator: ProviderRuntimeCoordinator,
    pub shared: Arc<LocalRuntimeShared>,
    pub truth: LocalTruthSeam,
    pub lifecycle: LocalLifecycleSeam,
    pub probe: LocalProbeSeam,
    pub store: FileRuntimeStore,
    pub state: ProviderRuntimeState,
    pub processes: Vec<solstone_core_system::provider_runtime::ManagedProcess>,
    pub launch_recorded_for: Option<String>,
    pub fixture_launch: Option<LocalFixtureLaunch>,
}

pub(crate) struct LocalFixtureLaunch {
    pub binary_path: String,
    pub model_id: String,
    pub model_path: String,
}

pub(crate) struct ParakeetProvider {
    pub coordinator: ProviderRuntimeCoordinator,
    pub shared: Arc<ParakeetRuntimeShared>,
    pub truth: ParakeetTruthSeam,
    pub lifecycle: ParakeetLifecycleSeam,
    pub probe: ParakeetProbeSeam,
    pub store: FileRuntimeStore,
    pub state: ProviderRuntimeState,
    pub processes: Vec<solstone_core_system::provider_runtime::ManagedProcess>,
}

pub(crate) struct SupervisorOutcome {
    pub lifecycle: SupervisorLifecycle,
    pub state: SupervisorState,
    pub regime: ShutdownRegime,
    pub stop_reason: tick::SupervisorStopReason,
}

#[derive(Debug)]
pub(crate) enum RuntimeBootError {
    Startup(String),
    BootstrapRecoveryRequired(ParentLossCoordinatorBootstrapFailure),
    SyncScan(solstone_core_system::lifecycle::SyncScanFailure),
    AdmissionWaitTerminal,
    ParentLostBeforeReadiness(ParentLossReason),
}

impl std::fmt::Display for RuntimeBootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup(error) => formatter.write_str(error),
            Self::BootstrapRecoveryRequired(reason) => {
                write!(
                    formatter,
                    "parent-loss coordinator bootstrap recovery required: {reason}"
                )
            }
            Self::SyncScan(error) => write!(formatter, "sync scan failed: {error}"),
            Self::AdmissionWaitTerminal => {
                formatter.write_str("post-publication heartbeat conflict")
            }
            Self::ParentLostBeforeReadiness(reason) => {
                write!(formatter, "parent lost before readiness: {reason:?}")
            }
        }
    }
}

impl SupervisorState {
    pub(crate) fn into_shutdown_driver(self, regime: ShutdownRegime) -> SupervisorShutdownDriver {
        SupervisorShutdownDriver::new(
            self,
            tokio::runtime::Handle::current(),
            matches!(regime, ShutdownRegime::ParentLossBounded),
            Arc::new(StderrBoundedShutdownDiagnosticSink),
        )
    }

    pub(crate) fn reap_managed(&mut self) {
        // TaskQueue owns its worker reaping; provider reconciliation marks owned
        // process records non-running once their cleanup result is committed.
        self.local.processes.retain(|process| process.running);
        self.parakeet.processes.retain(|process| process.running);
        for app in &mut self.app_processes {
            let exited = match app.process.as_mut() {
                Some(process) => match process.poll() {
                    Ok(Some(_)) => {
                        process.cleanup();
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        eprintln!(
                            "supervisor: failed to poll {} during reap: {error}",
                            app.service.as_str()
                        );
                        false
                    }
                },
                None => false,
            };
            if exited {
                app.process = None;
                app.started_at = None;
                app.restart_at = None;
            }
        }
    }

    pub(crate) fn reap_managed_until(&mut self, deadline: Instant) -> bool {
        self.local.processes.retain(|process| process.running);
        self.parakeet.processes.retain(|process| process.running);
        let mut completed = true;
        for app in &mut self.app_processes {
            if Instant::now() >= deadline {
                completed = false;
                break;
            }
            let exited = match app.process.as_mut() {
                Some(process) => match process.poll() {
                    Ok(Some(_)) => {
                        if !process.cleanup_until(deadline) {
                            completed = false;
                        }
                        true
                    }
                    Ok(None) => false,
                    Err(error) => {
                        eprintln!(
                            "supervisor: failed to poll {} during reap: {error}",
                            app.service.as_str()
                        );
                        false
                    }
                },
                None => false,
            };
            if exited {
                app.process = None;
                app.started_at = None;
                app.restart_at = None;
            }
        }
        completed
    }
}

fn app_fixture_binary() -> Option<String> {
    if std::env::var(APP_FIXTURE_ENABLED_ENV).as_deref() != Ok("1") {
        return None;
    }
    std::env::var(APP_FIXTURE_BINARY_ENV).ok()
}

fn convey_readiness_probe(
    fixture_binary: Option<&str>,
    fast_fixture_timing: bool,
) -> Box<dyn ConveyReadinessProbe> {
    if fixture_binary.is_some() {
        return Box::new(FixtureConveyReadinessProbe {
            fast_timing: fast_fixture_timing,
        });
    }
    Box::new(TcpConveyReadinessProbe)
}

fn app_processes(
    options: &SupervisorOptions,
    journal: &Path,
    fixture_binary: Option<&str>,
    journal_binary: Option<&Path>,
    convey_port: u16,
    fast_fixture_timing: bool,
) -> Vec<ManagedAppProcess> {
    let remote = options.remote.as_deref().is_some_and(|url| !url.is_empty());
    let services = vec![
        (AppService::Convey, !remote && !options.no_convey),
        (AppService::Sense, !remote),
        (AppService::Cortex, !remote && !options.no_cortex),
        (AppService::Spl, !remote && !options.no_spl),
    ];
    #[cfg(all(unix, feature = "journal-mcp-endpoint"))]
    let services = {
        let mut services = services;
        if !remote && mcp_endpoint_enabled(journal) {
            services.push((AppService::Mcp, true));
        }
        services
    };
    services
        .into_iter()
        .map(|(service, enabled)| {
            ManagedAppProcess::new(
                service,
                enabled,
                journal,
                fixture_binary,
                journal_binary,
                convey_port,
                fast_fixture_timing,
            )
        })
        .collect()
}

#[cfg(all(unix, feature = "journal-mcp-endpoint"))]
fn mcp_endpoint_enabled(journal: &Path) -> bool {
    matches!(
        read_journal_config(journal),
        Ok(config) if matches!(mcp_endpoint_capability(&config), Ok(McpEndpointCapability::Enabled))
    )
}

pub(crate) fn spawn_app_process(
    app: &mut ManagedAppProcess,
    journal: &Path,
    sink: Arc<CallosumSocketServer>,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> Result<(), String> {
    if app.service == AppService::Convey {
        if app.direct_door_generation.is_some() {
            return Err("previous direct-door cleanup authority is still retained".to_owned());
        }
        let generation = peek_direct_door_generation(journal)
            .map_err(|error| format!("failed to retain direct-door cleanup authority: {error}"))?;
        app.direct_door_generation = Some(generation);
    }
    let ledger = ParentLossLedger::open(journal)
        .map_err(|error| format!("could not open parent-loss lifecycle: {error}"))?;
    let active = ledger
        .active_generation()
        .map_err(|error| format!("could not read parent-loss lifecycle: {error}"))?
        .ok_or_else(|| "parent-loss coordinator has no active generation".to_owned())?;
    let launch_id = format!(
        "{}-{}-{}",
        app.service.as_str(),
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos())
    );
    let mut environment = BTreeMap::from([(
        OsString::from("SOL_SUPERVISOR_SPAWNED"),
        OsString::from("1"),
    )]);
    // Only Sense can reach transcription (it spawns native `journal
    // transcribe` children); Cortex talent workers cannot (denied by the
    // cogitate CLI allowlist), and Convey/Spl/Mcp never transcribe.
    if app.service == AppService::Sense {
        environment.extend(sense_child_environment.clone());
    }
    let authority = launch_managed_hosted(
        Disposition::InheritedParentScope,
        ManagedLaunchRequest {
            command: app.argv.clone(),
            options: SpawnOptions {
                journal_root: journal.to_path_buf(),
                reference: format!("supervisor-app-{}", app.service.as_str()),
                day: None,
                sink: Some(Arc::new(SupervisorProcessSink { server: sink })),
                environment,
            },
        },
        HostedLaunchProvenance {
            journal: journal.to_path_buf(),
            generation: active.generation,
            launch_id,
            service: Some(app.service.hosted_service_kind()),
            parent_launch_id: None,
            acknowledgement_timeout: PARENT_LOSS_CHILD_ADMISSION_TIMEOUT,
        },
    )
    .map_err(|error| error.to_string())?;
    let process = authority
        .into_managed()
        .map_err(|error| error.to_string())?;
    app.process = Some(process);
    app.started_at = Some(Instant::now());
    app.restart_at = None;
    if app.backoff.is_none() {
        clear_backoff_record(journal, app.service);
    }
    Ok(())
}

fn start_app_process(
    app: &mut ManagedAppProcess,
    journal: &Path,
    sink: Arc<CallosumSocketServer>,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) {
    if let Err(error) = spawn_app_process(app, journal, sink, sense_child_environment) {
        eprintln!(
            "supervisor: failed to start {}: {error}",
            app.service.as_str()
        );
        apply_app_exit(app, journal, AppExit::SpawnFailure);
    }
}

async fn wait_for_convey_ready(
    app: &mut ManagedAppProcess,
    journal: &Path,
    probe: &dyn ConveyReadinessProbe,
    lifecycle: &mut SupervisorLifecycle,
    heartbeat_interval: Duration,
) -> Result<bool, SyncTickOutcome> {
    let start = Instant::now();
    renew_pre_ready_heartbeat(lifecycle)?;
    let mut last_heartbeat = Instant::now();
    loop {
        let exited = match app.process.as_mut() {
            Some(process) => match process.poll() {
                Ok(Some(exit_code)) => {
                    process.cleanup();
                    Some(exit_code)
                }
                Ok(None) => None,
                Err(error) => {
                    eprintln!(
                        "supervisor: failed to poll convey during startup: {error}; continuing into supervise loop"
                    );
                    return Ok(false);
                }
            },
            None => return Ok(false),
        };
        if let Some(exit_code) = exited {
            apply_app_exit(app, journal, AppExit::Process { code: exit_code });
            eprintln!(
                "supervisor: convey exited during startup (exit {exit_code}); continuing into supervise loop"
            );
            return Ok(false);
        }
        if probe.is_ready(journal, &app.argv) {
            return Ok(true);
        }
        if last_heartbeat.elapsed() >= heartbeat_interval {
            renew_pre_ready_heartbeat(lifecycle)?;
            last_heartbeat = Instant::now();
        }
        if start.elapsed() >= probe.wait_window() {
            eprintln!(
                "supervisor: convey was not ready during startup; continuing into supervise loop"
            );
            return Ok(false);
        }
        tokio::time::sleep(probe.poll_interval()).await;
    }
}

fn renew_pre_ready_heartbeat(lifecycle: &mut SupervisorLifecycle) -> Result<(), SyncTickOutcome> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |value| value.as_secs_f64());
    match lifecycle.tick_sync(None, now) {
        SyncTickOutcome::Healthy => Ok(()),
        outcome => Err(outcome),
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_app_stack(
    app_processes: &mut [ManagedAppProcess],
    journal: &Path,
    sink: Arc<CallosumSocketServer>,
    probe: &dyn ConveyReadinessProbe,
    lifecycle: &mut SupervisorLifecycle,
    heartbeat_interval: Duration,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> Result<(), SyncTickOutcome> {
    let services = vec![
        AppService::Convey,
        AppService::Sense,
        AppService::Cortex,
        AppService::Spl,
    ];
    #[cfg(all(unix, feature = "journal-mcp-endpoint"))]
    let services = {
        let mut services = services;
        if app_processes
            .iter()
            .any(|app| app.service == AppService::Mcp)
        {
            services.push(AppService::Mcp);
        }
        services
    };
    for service in services {
        let app = app_processes
            .iter_mut()
            .find(|app| app.service == service)
            .expect("app process inventory is complete");
        if !app.enabled {
            continue;
        }
        start_app_process(app, journal, sink.clone(), sense_child_environment);
        if service == AppService::Convey && app.process.is_some() {
            let _ =
                wait_for_convey_ready(app, journal, probe, lifecycle, heartbeat_interval).await?;
        }
    }
    Ok(())
}

pub(crate) async fn boot_and_tick(
    lifecycle: PreReadySupervisorLifecycle,
    journal: PathBuf,
    options: SupervisorOptions,
    journal_binary: Option<PathBuf>,
    parent_watch: Option<ParentWatch>,
    sense_child_environment: BTreeMap<OsString, OsString>,
) -> Result<SupervisorOutcome, RuntimeBootError> {
    let mut lifecycle = lifecycle.into_lifecycle();
    let mut shutdown_signals = match tick::ShutdownSignals::install() {
        Ok(signals) => signals,
        Err(error) => return Err(abort_pre_ready(&lifecycle, error)),
    };
    let server = Arc::new(
        match CallosumSocketServer::bind(journal.join("health/callosum.sock")).await {
            Ok(server) => server,
            Err(error) => return Err(abort_pre_ready(&lifecycle, error)),
        },
    );
    let mut connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), serde_json::Map::new());
    connection.start();
    if let Err(error) = wait_for_callosum_connection(&mut connection).await {
        connection.stop().await;
        server.stop().await;
        return Err(abort_pre_ready(&lifecycle, error));
    }
    let default_cap = std::env::var("SOLSTONE_SUPERVISOR_TASK_CAP_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TASK_MAX_RUNTIME);
    let parakeet_shared = Arc::new(ParakeetRuntimeShared::default());
    let queue = TaskQueue::new(TaskQueueOptions {
        journal_root: journal.clone(),
        cap_resolver: Arc::new(DefaultCapResolver::new(default_cap)),
        process_state_probe: Arc::new(SystemProcessStateProbe),
        queue_sink: Some(Arc::new(SupervisorTaskQueueSink(Arc::clone(&server)))),
        process_sink: Some(Arc::new(SupervisorProcessSink {
            server: Arc::clone(&server),
        })),
        ready: false,
        before_deadline_commit: None,
        child_environment: sense_child_environment.clone(),
    });
    let clock: Arc<dyn solstone_core_system::provider_runtime::RuntimeClock> =
        Arc::new(SystemRuntimeClock::default());
    let remote = options.remote.as_deref().is_some_and(|url| !url.is_empty());
    let local_shared = Arc::new(LocalRuntimeShared::default());
    let fixture_truth = std::env::var("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE").as_deref() == Ok("1");
    // This pair is a test-only seam. Production must use LocalTruthSeam's
    // artifact-derived launch request rather than synthetic model or GPU data.
    let fixture_launch = if fixture_truth {
        std::env::var_os("SOLSTONE_LOCAL_BINARY").map(|binary_path| LocalFixtureLaunch {
            binary_path: PathBuf::from(binary_path).display().to_string(),
            model_id: std::env::var("SOLSTONE_LOCAL_MODEL_ID")
                .unwrap_or_else(|_| "local/test".to_owned()),
            model_path: std::env::var("SOLSTONE_LOCAL_MODEL_PATH")
                .unwrap_or_else(|_| "test-ready".to_owned()),
        })
    } else {
        None
    };
    let truth = if fixture_launch.is_some() && fixture_truth {
        LocalTruthSeam::with_config(
            local_shared.clone(),
            LocalTruthConfig {
                journal_path: journal.clone(),
                // The fixture path deliberately uses a lightweight native Metal
                // artifact proof instead of inspecting host NVIDIA hardware. Its launch
                // request is replaced below with the synthetic CUDA fixture plan.
                platform: Platform::Darwin,
                nvidia_probe: None,
                vulkan_devices: Vec::new(),
            },
        )
    } else {
        LocalTruthSeam::new(local_shared.clone(), journal.clone())
    };
    let local = LocalProvider {
        coordinator: ProviderRuntimeCoordinator::new(),
        shared: local_shared.clone(),
        truth,
        lifecycle: LocalLifecycleSeam::new(local_shared.clone(), clock.clone()),
        probe: LocalProbeSeam::new(local_shared.clone(), journal.clone()),
        store: FileRuntimeStore::new(
            journal.clone(),
            ProviderName::Local,
            local_shared.clone(),
            clock.clone(),
        ),
        state: ProviderRuntimeState::new(ProviderName::Local),
        processes: Vec::new(),
        launch_recorded_for: None,
        fixture_launch,
    };
    // The four blockers that once made this a seeded stub are CLOSED. Keeping
    // the contract here, satisfied rather than deleted, because it is what a
    // cutover wave reads to decide whether it may proceed:
    //   - Installed-artifact resolution: `ParakeetTruthSeam` derives pinned
    //     binary and model paths and verifies each resolves to a regular file.
    //     No bare PATH-resolved names.
    //   - Device selection: `ParakeetTruthSeam::new` fills Vulkan devices from
    //     the packaged probe, prefers hardware over software ICDs, and WARNS
    //     when auto cannot honour a GPU. Missing Vulkan binaries stay on CPU.
    //   - Thread count: `parakeet_physical_thread_count()` — physical cores
    //     from `/proc/cpuinfo` on Linux, with a documented fallback. Not a
    //     constant.
    //   - Truth seam: a real `ParakeetTruthSeam` replaces `NoopWorkers`, and
    //     the `next_truth_at: f64::MAX` sentinel is gone, so desired-vs-actual
    //     state is re-observed on the tick like Local's.
    //
    // ⚠ STILL NARROWER THAN PYTHON, and deliberately — see the module header on
    // `provider_runtime::parakeet_truth_seam`. It does not yet inspect
    // manifests, proof state, install progress, or binary host eligibility, and
    // native Vulkan enumeration plus the `decide_parakeet_auto_placement` /
    // `is_local_provider_needed` co-location branch remain follow-up work. Those
    // gaps degrade placement quality; they do not leave the provider unmanaged,
    // which is the distinction that gated the cutover.
    let parakeet_fixture = std::env::var(PARAKEET_FIXTURE_ENV).as_deref() == Ok("1");
    let parakeet_truth = if parakeet_fixture {
        ParakeetTruthSeam::with_config(
            parakeet_shared.clone(),
            ParakeetTruthConfig {
                journal_path: journal.clone(),
                remote_mode: false,
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
    } else {
        ParakeetTruthSeam::new(parakeet_shared.clone(), journal.clone(), remote)
    };
    let parakeet = ParakeetProvider {
        coordinator: ProviderRuntimeCoordinator::new(),
        shared: parakeet_shared.clone(),
        truth: parakeet_truth,
        lifecycle: ParakeetLifecycleSeam::new(parakeet_shared.clone(), clock.clone()),
        probe: ParakeetProbeSeam::new(parakeet_shared.clone(), journal.clone()),
        store: FileRuntimeStore::new(
            journal.clone(),
            ProviderName::Parakeet,
            parakeet_shared.clone(),
            clock,
        ),
        state: ProviderRuntimeState::new(ProviderName::Parakeet),
        processes: Vec::new(),
    };
    let wall = chrono::Local::now();
    let now = ScheduleNow {
        local: wall.naive_local(),
        unix_millis: wall.timestamp_millis(),
    };
    let scheduler = if options.no_schedule {
        None
    } else {
        let schedule_config_path = journal.join("config/schedules.json");
        if let Err(error) = initialize_schedule_config(&schedule_config_path) {
            return Err(
                abort_published_setup(&lifecycle, &queue, &mut connection, &server, error).await,
            );
        }
        let mut scheduler = match ScheduleEngine::init(
            schedule_config_path,
            journal.join("health/scheduler.json"),
            now,
        ) {
            Ok((scheduler, _)) => scheduler,
            Err(error) => {
                return Err(abort_published_setup(
                    &lifecycle,
                    &queue,
                    &mut connection,
                    &server,
                    error,
                )
                .await);
            }
        };
        let schedule_sink = SupervisorScheduleSink {
            queue: queue.clone(),
            server: server.clone(),
        };
        let _ = scheduler.catch_up(now, &schedule_sink);
        Some(scheduler)
    };
    let fixture_binary = app_fixture_binary();
    let fast_fixture_timing = fixture_binary.is_some()
        && std::env::var(APP_FIXTURE_FAST_TIMING_ENV).as_deref() == Ok("1");
    let readiness_probe = convey_readiness_probe(fixture_binary.as_deref(), fast_fixture_timing);
    let convey_port = if options.port != 0 {
        options.port
    } else {
        match resolve_available_port() {
            Ok(port) => port,
            Err(error) => {
                return Err(abort_published_setup(
                    &lifecycle,
                    &queue,
                    &mut connection,
                    &server,
                    error,
                )
                .await);
            }
        }
    };
    let app_processes = app_processes(
        &options,
        &journal,
        fixture_binary.as_deref(),
        journal_binary.as_deref(),
        convey_port,
        fast_fixture_timing,
    );
    let supervisor_generation = match current_supervisor_instance() {
        Ok(instance) => instance,
        Err(error) => {
            return Err(
                abort_published_setup(&lifecycle, &queue, &mut connection, &server, error).await,
            );
        }
    };
    let supervisor_heartbeat_filename = lifecycle.heartbeat_filename().to_owned();
    let parent_loss_coordinator = match bootstrap_parent_loss_coordinator(
        &journal,
        supervisor_generation,
        app_processes
            .iter()
            .filter(|app| app.enabled)
            .map(|app| app.service.hosted_service_kind())
            .collect(),
        supervisor_heartbeat_filename,
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            return Err(abort_published_setup_with_error(
                &lifecycle,
                &queue,
                &mut connection,
                &server,
                RuntimeBootError::BootstrapRecoveryRequired(error),
            )
            .await);
        }
    };
    let direct_port = match selected_direct_door_port(&journal, options.direct_port) {
        Ok(port) => port,
        Err(error) => {
            return Err(
                abort_published_setup(&lifecycle, &queue, &mut connection, &server, error).await,
            );
        }
    };
    if let Err(error) = persist_direct_door_port(&journal, direct_port) {
        return Err(
            abort_published_setup(&lifecycle, &queue, &mut connection, &server, error).await,
        );
    }
    if let Err(error) = initialize_direct_door(&journal, direct_port) {
        return Err(
            abort_published_setup(&lifecycle, &queue, &mut connection, &server, error).await,
        );
    }
    let mut state = SupervisorState {
        journal,
        is_remote_mode: remote,
        no_daily: options.no_daily,
        server,
        connection,
        queue,
        last_sync_snapshot: None,
        stale_heartbeats: Vec::new(),
        shutdown_started: AtomicBool::new(false),
        started: Instant::now(),
        scheduler,
        recorded_schedule_completions: BTreeSet::new(),
        app_processes,
        local,
        parakeet,
        flush: FlushState::default(),
        daily: DailyState {
            last_day: Some(chrono::Local::now().date_naive()),
        },
        // Startup reconciliation/drain below seeds the retry watermark.
        last_retry_expiry_drain: Instant::now(),
        wedge: WedgeState::default(),
        timing: SupervisorTiming::for_app_fixture(fast_fixture_timing),
        parent_loss_coordinator: Some(parent_loss_coordinator),
        sense_child_environment,
        #[cfg(windows)]
        parakeet_sense_credentials_revision: 0,
    };
    let startup_journal = state.journal.clone();
    let startup_server = Arc::clone(&state.server);
    let pre_ready_heartbeat_interval = if fast_fixture_timing {
        FAST_FIXTURE_PRE_READY_HEARTBEAT_INTERVAL
    } else {
        Duration::from_secs_f64(DEFAULT_INTERVAL_SECONDS)
    };
    let startup_sense_child_environment = state.sense_child_environment.clone();
    if let Err(outcome) = start_app_stack(
        &mut state.app_processes,
        &startup_journal,
        startup_server,
        readiness_probe.as_ref(),
        &mut lifecycle,
        pre_ready_heartbeat_interval,
        &startup_sense_child_environment,
    )
    .await
    {
        let startup = classify_pre_ready_sync_error(outcome);
        return Err(abort_pre_ready_state(&mut state, &lifecycle, startup).await);
    }
    pause_before_final_parent_check().await;
    if let Some(watch) = parent_watch
        && let ParentWatchStatus::Lost(reason) =
            watch.check(&solstone_core_system::process::SystemProcessInstanceSource)
    {
        return Err(abort_pre_ready_state(
            &mut state,
            &lifecycle,
            RuntimeBootError::ParentLostBeforeReadiness(reason),
        )
        .await);
    }
    if let Err(outcome) = renew_pre_ready_heartbeat(&mut lifecycle) {
        let startup = classify_pre_ready_sync_error(outcome);
        return Err(abort_pre_ready_state(&mut state, &lifecycle, startup).await);
    }
    if let Err(error) = lifecycle.signal_ready(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |value| value.as_secs_f64()),
        serde_json::Map::new(),
    ) {
        let startup = RuntimeBootError::Startup(error.to_string());
        return Err(abort_pre_ready_state(&mut state, &lifecycle, startup).await);
    }
    state.queue.set_ready();
    if let Err(error) = tick::initialize_catchup(
        &state.journal,
        &state.queue,
        state.is_remote_mode,
        state.no_daily,
        chrono::Local::now().date_naive(),
        SystemTime::now(),
    ) {
        eprintln!("supervisor: startup catchup reconciliation failed: {error}");
    }
    state.last_retry_expiry_drain = Instant::now();
    let stop_reason = tick::run(
        &mut state,
        &mut lifecycle,
        &mut shutdown_signals,
        parent_watch,
    )
    .await;
    let regime = shutdown_regime_for(&stop_reason);
    Ok(SupervisorOutcome {
        lifecycle,
        state,
        regime,
        stop_reason,
    })
}

fn shutdown_regime_for(stop_reason: &tick::SupervisorStopReason) -> ShutdownRegime {
    match stop_reason {
        tick::SupervisorStopReason::ParentLost(_) => ShutdownRegime::ParentLossBounded,
        tick::SupervisorStopReason::Signal(_) | tick::SupervisorStopReason::Sync(_) => {
            ShutdownRegime::Standard
        }
    }
}

async fn pause_before_final_parent_check() {
    let Ok(marker) = std::env::var("SOLSTONE_SUPERVISOR_HOSTED_PAUSE_BEFORE_FINAL_PARENT_CHECK")
    else {
        return;
    };
    if std::env::var("SOLSTONE_SUPERVISOR_APP_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let marker = PathBuf::from(marker);
    if std::fs::write(&marker, b"paused\n").is_err() {
        return;
    }
    let go = PathBuf::from(format!("{}.go", marker.display()));
    while !go.exists() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn abort_published_setup(
    lifecycle: &SupervisorLifecycle,
    queue: &TaskQueue,
    connection: &mut CallosumSocketConnection,
    server: &Arc<CallosumSocketServer>,
    error: impl std::fmt::Display,
) -> RuntimeBootError {
    abort_published_setup_with_error(
        lifecycle,
        queue,
        connection,
        server,
        RuntimeBootError::Startup(error.to_string()),
    )
    .await
}

async fn abort_published_setup_with_error(
    lifecycle: &SupervisorLifecycle,
    queue: &TaskQueue,
    connection: &mut CallosumSocketConnection,
    server: &Arc<CallosumSocketServer>,
    startup: RuntimeBootError,
) -> RuntimeBootError {
    let queue_report = queue.shutdown();
    connection.stop().await;
    server.stop().await;
    let mut cleanup_failures = Vec::new();
    if queue_report.active_count != 0 || queue_report.forced {
        cleanup_failures.push(format!(
            "pre-ready queue shutdown observed {} active tasks (forced={})",
            queue_report.active_count, queue_report.forced
        ));
    }
    if cleanup_failures.is_empty() {
        if let Err(error) = lifecycle.abort_before_ready() {
            cleanup_failures.push(format!("lifecycle cleanup failed: {error}"));
        }
    } else {
        cleanup_failures.push(
            "lifecycle heartbeat and identity retained because runtime cleanup was not established"
                .to_owned(),
        );
    }
    cleanup_result(startup, cleanup_failures)
}

async fn teardown_pre_ready_state(state: &mut SupervisorState) -> Result<(), String> {
    let mut failures = Vec::new();
    let queue_report = state.queue.shutdown();
    if queue_report.active_count != 0 || queue_report.forced {
        failures.push(format!(
            "queue shutdown observed {} active tasks (forced={})",
            queue_report.active_count, queue_report.forced
        ));
    }
    for app in state.app_processes.iter_mut().rev() {
        if let Some(process) = app.process.as_mut() {
            if let Err(error) =
                process.terminate_exact(solstone_core_system::process::SERVICE_SHUTDOWN_TIMEOUT)
            {
                failures.push(format!(
                    "{} exact-process termination failed: {error}",
                    app.service.as_str()
                ));
            }
            process.cleanup();
        }
        if let Err(error) = withhold_app_direct_door(app, &state.journal) {
            failures.push(format!(
                "{} direct-door cleanup failed: {error}",
                app.service.as_str()
            ));
        }
    }
    state.connection.stop().await;
    state.server.stop().await;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

async fn abort_pre_ready_state(
    state: &mut SupervisorState,
    lifecycle: &SupervisorLifecycle,
    startup: RuntimeBootError,
) -> RuntimeBootError {
    let mut cleanup_failures = Vec::new();
    match teardown_pre_ready_state(state).await {
        Ok(()) => {
            if let Err(error) = lifecycle.abort_before_ready() {
                cleanup_failures.push(format!("lifecycle cleanup failed: {error}"));
            }
        }
        Err(error) => {
            cleanup_failures.push(format!("runtime cleanup failed: {error}"));
            cleanup_failures.push(
                "lifecycle heartbeat and identity retained because process cleanup was not established"
                    .to_owned(),
            );
        }
    }
    cleanup_result(startup, cleanup_failures)
}

fn cleanup_result(startup: RuntimeBootError, cleanup_failures: Vec<String>) -> RuntimeBootError {
    if cleanup_failures.is_empty() {
        startup
    } else {
        RuntimeBootError::Startup(format!(
            "{startup}; pre-ready cleanup failed: {}",
            cleanup_failures.join("; ")
        ))
    }
}

fn classify_pre_ready_sync_error(outcome: SyncTickOutcome) -> RuntimeBootError {
    match outcome {
        SyncTickOutcome::Healthy => unreachable!("healthy pre-ready heartbeat renewal continues"),
        SyncTickOutcome::Conflict(_) => RuntimeBootError::AdmissionWaitTerminal,
        SyncTickOutcome::CompleteScanFailure(failure) => RuntimeBootError::SyncScan(failure),
        outcome => {
            RuntimeBootError::Startup(format!("pre-ready heartbeat renewal failed: {outcome:?}"))
        }
    }
}

fn abort_pre_ready(
    lifecycle: &SupervisorLifecycle,
    error: impl std::fmt::Display,
) -> RuntimeBootError {
    let error = error.to_string();
    match lifecycle.abort_before_ready() {
        Ok(()) => RuntimeBootError::Startup(error),
        Err(cleanup) => RuntimeBootError::Startup(format!(
            "{error}; pre-ready lifecycle cleanup failed: {cleanup}"
        )),
    }
}

async fn wait_for_callosum_connection(
    connection: &mut CallosumSocketConnection,
) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
        .to_string();
    tokio::time::timeout(CALLOSUM_CONNECTION_READY_WINDOW, async {
        loop {
            let _ = connection.emit(
                "__solstone_internal",
                "connection_ready",
                serde_json::Map::from_iter([(
                    "nonce".to_owned(),
                    serde_json::Value::String(nonce.clone()),
                )]),
            );
            tokio::select! {
                message = connection.next_message() => {
                    let Some(message) = message else {
                        return Err("supervisor Callosum connection stopped before becoming ready".to_owned());
                    };
                    if message.tract == "__solstone_internal"
                        && message.event == "connection_ready"
                        && message.extra.get("nonce").and_then(serde_json::Value::as_str) == Some(nonce.as_str())
                    {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(CALLOSUM_CONNECTION_READY_INTERVAL) => {}
            }
        }
    })
    .await
    .map_err(|_| "supervisor Callosum connection did not become ready".to_owned())??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AppService, JournalBinaryPreflightError, ManagedAppProcess,
        ParentLossCoordinatorBootstrapFailure, ParentLossCoordinatorBootstrapTestFault,
        ParentLossCoordinatorSession, RuntimeBootError, bootstrap_parent_loss_coordinator,
        cleanup_result, parent_loss_coordinator_bootstrap_test_spawned,
        parent_loss_coordinator_launch_request, resolve_journal_binary_from,
        selected_direct_door_port, set_parent_loss_coordinator_bootstrap_test_fault,
        shutdown_regime_for, validate_journal_binary, withhold_app_direct_door,
    };
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use solstone_core_system::lifecycle::{
        CoordinatorBootstrap, DeclaredParent, ParentLossCoordinator, ParentLossLedger,
        ParentLossReason, ParentLossTerminalDisposition, ShutdownRegime, SyncTickOutcome,
    };
    use solstone_core_system::process::{
        InstanceVerdict, ProcessInstanceSource, SystemProcessInstanceSource,
    };

    use super::super::tick::{SupervisorSignal, SupervisorStopReason};

    #[test]
    fn resolves_journal_binary_from_executable_directory() {
        assert_eq!(
            resolve_journal_binary_from(Path::new("/foo/bar")),
            PathBuf::from("/foo/bar/solstone-core-journal")
        );
    }

    #[test]
    fn parent_loss_coordinator_has_an_independent_process_group() {
        let supervisor = DeclaredParent::capture_current()
            .expect("live supervisor identity")
            .instance();
        let request = parent_loss_coordinator_launch_request(
            Path::new("/tmp/parent-loss-coordinator-process-group"),
            supervisor,
            &[],
            "solstone-v2-test-test.check",
        )
        .expect("coordinator launch request");

        assert!(request.process_group);
        assert!(
            request
                .arguments
                .windows(2)
                .any(|pair| pair[0] == "--supervisor-heartbeat"
                    && pair[1] == "solstone-v2-test-test.check"),
            "the coordinator receives the exact supervisor heartbeat it may later claim"
        );
    }

    #[test]
    fn supervisor_wait_observes_live_coordinator_retirement_acknowledgement() {
        let journal = tempfile::TempDir::new().expect("temporary journal");
        let supervisor = DeclaredParent::capture_current()
            .expect("live direct parent")
            .instance();
        let capability = b"supervisor-retirement-ack-test-capability".to_vec();
        let (coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: journal.path().to_path_buf(),
            supervisor,
            enabled: Vec::new(),
            supervisor_heartbeat_filename: "solstone-v2-test-test.check".to_owned(),
            capability: capability.clone(),
        })
        .expect("coordinator bootstrap");
        let session = ParentLossCoordinatorSession {
            generation: coordinator.generation(),
            supervisor,
            capability,
        };

        session
            .write_retire_expected(journal.path())
            .expect("authenticated retirement request");
        let coordinator = std::thread::spawn(move || coordinator.run());

        assert!(
            session
                .wait_for_retire_expected_ack(journal.path(), Duration::from_secs(2))
                .expect("read coordinator acknowledgement")
        );
        assert_eq!(
            coordinator
                .join()
                .expect("coordinator thread")
                .expect("coordinator retirement"),
            ParentLossTerminalDisposition::RetiredExpected
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_faults_refuse_before_service_side_effects_and_reap_spawned_coordinator() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            for (fault, expected) in [
                (
                    ParentLossCoordinatorBootstrapTestFault::Launch,
                    ParentLossCoordinatorBootstrapFailure::Launch,
                ),
                (
                    ParentLossCoordinatorBootstrapTestFault::IdentityEstablishment,
                    ParentLossCoordinatorBootstrapFailure::IdentityEstablishment,
                ),
                (
                    ParentLossCoordinatorBootstrapTestFault::InitialAdmissionHandshake,
                    ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake,
                ),
            ] {
                let journal = tempfile::TempDir::new().expect("temporary journal");
                let supervisor = DeclaredParent::capture_current()
                    .expect("live supervisor identity")
                    .instance();
                set_parent_loss_coordinator_bootstrap_test_fault(Some(fault));
                let outcome = bootstrap_parent_loss_coordinator(
                    journal.path(),
                    supervisor,
                    vec![solstone_core_system::lifecycle::HostedServiceKind::Sense],
                    "solstone-v2-test-test.check".to_owned(),
                )
                .await;
                let spawned = parent_loss_coordinator_bootstrap_test_spawned();
                set_parent_loss_coordinator_bootstrap_test_fault(None);

                let reason = match outcome {
                    Err(reason) => reason,
                    Ok(_) => panic!("injected bootstrap failure unexpectedly admitted"),
                };
                assert_eq!(reason, expected);
                assert!(matches!(
                    RuntimeBootError::BootstrapRecoveryRequired(reason),
                    RuntimeBootError::BootstrapRecoveryRequired(recovery) if recovery == expected
                ));
                assert!(
                    ParentLossLedger::open(journal.path())
                        .expect("fresh parent-loss ledger")
                        .active_generation()
                        .expect("read active generation")
                        .is_none(),
                    "bootstrap failure must precede admission and all coordinator state"
                );
                assert!(!journal.path().join("health/direct-door.json").exists());
                assert!(!journal.path().join("health/callosum.sock").exists());
                assert!(!journal.path().join("health/supervisor.ready").exists());
                assert!(
                    !journal
                        .path()
                        .join("health/parent-loss/bootstrap-ready.json")
                        .exists()
                );
                if let Some(identity) = spawned {
                    assert!(matches!(
                        SystemProcessInstanceSource.observe(&identity),
                        InstanceVerdict::NotSameOrExited
                    ));
                } else {
                    assert_eq!(fault, ParentLossCoordinatorBootstrapTestFault::Launch);
                }
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn sibling_preflight_requires_the_co_located_binary_to_be_executable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().expect("temporary executable directory");
        let sibling = resolve_journal_binary_from(directory.path());
        assert!(matches!(
            validate_journal_binary(sibling.clone()),
            Err(JournalBinaryPreflightError::MissingOrNotExecutable { path }) if path == sibling
        ));

        std::fs::write(&sibling, b"fixture").expect("non-executable sibling");
        std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o600))
            .expect("non-executable mode");
        assert!(matches!(
            validate_journal_binary(sibling.clone()),
            Err(JournalBinaryPreflightError::MissingOrNotExecutable { path }) if path == sibling
        ));

        std::fs::set_permissions(&sibling, std::fs::Permissions::from_mode(0o700))
            .expect("executable mode");
        assert_eq!(validate_journal_binary(sibling.clone()).unwrap(), sibling);
    }

    #[test]
    fn restart_without_direct_port_retains_the_persisted_port() {
        let journal = tempfile::TempDir::new().expect("temporary journal");
        std::fs::create_dir_all(journal.path().join("config")).expect("config directory");
        std::fs::write(
            journal.path().join("config/journal.json"),
            r#"{"pairing":{"direct_port":9000}}"#,
        )
        .expect("config write");
        assert_eq!(
            selected_direct_door_port(journal.path(), None).unwrap(),
            9000
        );
        assert_eq!(
            selected_direct_door_port(journal.path(), Some(9001)).unwrap(),
            9001
        );
    }

    #[test]
    fn parent_loss_bounded_regime_is_selected_only_for_parent_lost_stop_reasons() {
        assert_eq!(
            shutdown_regime_for(&SupervisorStopReason::ParentLost(
                ParentLossReason::ExitedOrReused,
            )),
            ShutdownRegime::ParentLossBounded
        );
        assert_eq!(
            shutdown_regime_for(&SupervisorStopReason::Signal(SupervisorSignal::SigTerm)),
            ShutdownRegime::Standard
        );
        assert_eq!(
            shutdown_regime_for(&SupervisorStopReason::Sync(SyncTickOutcome::Healthy)),
            ShutdownRegime::Standard
        );
    }

    #[test]
    fn pre_ready_cleanup_failure_overrides_an_ordinary_refusal() {
        let outcome = cleanup_result(
            RuntimeBootError::ParentLostBeforeReadiness(ParentLossReason::ExitedOrReused),
            vec!["convey direct-door cleanup failed".to_owned()],
        );
        assert!(matches!(
            outcome,
            RuntimeBootError::Startup(message)
                if message.contains("pre-ready cleanup failed")
                    && message.contains("convey direct-door cleanup failed")
        ));
    }

    #[test]
    fn failed_direct_door_cleanup_retains_generation_authority() {
        let journal = tempfile::TempDir::new().expect("temporary journal");
        std::fs::create_dir_all(journal.path().join("config")).expect("config directory");
        std::fs::create_dir_all(journal.path().join("health")).expect("health directory");
        std::fs::write(
            journal.path().join("config/journal.json"),
            r#"{"pairing":{"direct_port":9000}}"#,
        )
        .expect("journal config");
        std::fs::write(journal.path().join("health/direct-door.json"), b"not JSON")
            .expect("corrupt direct-door fixture");
        let mut app = ManagedAppProcess::new(
            AppService::Convey,
            true,
            journal.path(),
            Some("/bin/true"),
            None,
            9000,
            false,
        );
        app.direct_door_generation = Some(7);

        assert!(withhold_app_direct_door(&mut app, journal.path()).is_err());
        assert_eq!(
            app.direct_door_generation,
            Some(7),
            "a failed cleanup remains retryable instead of consuming authority"
        );
    }

    #[test]
    fn admitted_lifecycle_precedes_callosum_and_queue_release_follows_readiness() {
        let source = include_str!("runtime.rs");
        let boot = source
            .split("pub(crate) async fn boot_and_tick(")
            .nth(1)
            .expect("boot_and_tick source")
            .split("fn shutdown_regime_for(")
            .next()
            .expect("boot_and_tick body");
        let admitted = boot
            .find("lifecycle.into_lifecycle()")
            .expect("final-admitted lifecycle");
        let callosum = boot
            .find("CallosumSocketServer::bind")
            .expect("Callosum bind");
        let queue = boot.find("TaskQueue::new").expect("queue construction");
        let readiness = boot
            .find("lifecycle.signal_ready")
            .expect("readiness write");
        let release = boot
            .find("state.queue.set_ready()")
            .expect("queue readiness release");

        assert!(
            admitted < callosum && callosum < queue,
            "only a final-admitted lifecycle may bind Callosum or construct the queue"
        );
        assert!(boot[queue..].contains("ready: false"));
        assert!(
            readiness < release,
            "queued work is released only after readiness"
        );
    }
}
