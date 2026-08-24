// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_cli::SupervisorOptions;
use solstone_core_journal_config::read_direct_door_port;
use solstone_core_journal_config_write::persist_direct_door_port;
use solstone_core_journal_io::{JsonWriteOptions, write_json};
use solstone_core_local::plan::Platform;
use solstone_core_system::cap::{DEFAULT_TASK_MAX_RUNTIME, DefaultCapResolver};
use solstone_core_system::direct_door::{
    initialize_direct_door, peek_direct_door_generation, withhold_direct_door,
};
use solstone_core_system::lifecycle::{
    ForeignWriter, ShutdownRegime, SupervisorLifecycle, SyncSnapshot,
};
use solstone_core_system::process::{
    ManagedProcess, RestartDecision, RestartPolicy, SpawnError, SpawnOptions, describe_exit,
};
use solstone_core_system::provider_runtime::{
    FileRuntimeStore, LocalLifecycleSeam, LocalProbeSeam, LocalRuntimeShared, LocalTruthConfig,
    LocalTruthSeam, ParakeetLifecycleSeam, ParakeetProbeSeam, ParakeetRuntimeShared,
    ParakeetTruthConfig, ParakeetTruthSeam, ProviderName, ProviderRuntimeCoordinator,
    ProviderRuntimeState, ReasonCode, RuntimePhase, SystemRuntimeClock, WedgeState,
};
use solstone_core_system::queue::{SystemProcessStateProbe, TaskQueue, TaskQueueOptions};
use solstone_core_system::schedule::{ScheduleEngine, ScheduleNow};
use solstone_core_system::status_wire::CrashedServiceCandidate;

use super::bus::{SupervisorProcessSink, SupervisorScheduleSink, SupervisorTaskQueueSink};
use super::shutdown::SupervisorShutdownDriver;
use super::tick;

const APP_FIXTURE_ENABLED_ENV: &str = "SOLSTONE_SUPERVISOR_APP_FIXTURE";
const APP_FIXTURE_BINARY_ENV: &str = "SOLSTONE_SUPERVISOR_APP_BINARY";
/// Enables short scheduling intervals for the app-process integration fixture.
/// This is only honored while the fixture binary itself is enabled.
const APP_FIXTURE_FAST_TIMING_ENV: &str = "SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING";
const PARAKEET_FIXTURE_ENV: &str = "SOLSTONE_SUPERVISOR_PARAKEET_FIXTURE";
/// Fixture Convey argv override; test-constructed paths must not contain spaces.
const APP_FIXTURE_CONVEY_ARGV_ENV: &str = "SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV";
const CONVEY_READY_WINDOW: Duration = Duration::from_secs(60);
const CONVEY_READY_INTERVAL: Duration = Duration::from_millis(100);
const CONVEY_READY_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
const FIXTURE_CONVEY_READY_WINDOW: Duration = Duration::from_secs(3);
const FIXTURE_CONVEY_READY_INTERVAL: Duration = Duration::from_millis(20);
const FAST_FIXTURE_CONVEY_READY_WINDOW: Duration = Duration::from_millis(100);
const FAST_FIXTURE_CONVEY_READY_INTERVAL: Duration = Duration::from_millis(5);
const CALLOSUM_CONNECTION_READY_WINDOW: Duration = Duration::from_secs(2);
const CALLOSUM_CONNECTION_READY_INTERVAL: Duration = Duration::from_millis(5);

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
    pub heartbeat_filename: String,
    pub stale_heartbeats: Vec<ForeignWriter>,
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
}

impl AppService {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Convey => "convey",
            Self::Sense => "sense",
            Self::Cortex => "cortex",
            Self::Spl => "spl",
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
        }
        argv
    }
}

pub(crate) struct TerminalState {
    pub reason: String,
    pub exit_code: Option<i32>,
    pub restart_attempts: u32,
}

pub(crate) enum AppExit {
    Process { code: i32 },
    SpawnFailure,
}

pub(crate) enum RestartRequestOutcome {
    Signaled { pid: u32 },
    Revived,
    Ignored,
}

#[derive(Serialize)]
struct FailedRecord {
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
    pub restart_requested: bool,
    /// Correlates an accepted app restart with all ensuing app-process events.
    pub restart_id: Arc<Mutex<Option<String>>>,
    pub terminal: Option<TerminalState>,
    /// Generation claimed from `health/direct-door.json` when this Convey child spawned.
    pub direct_door_generation: Option<u64>,
}

impl ManagedAppProcess {
    fn new(
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
            restart_requested: false,
            restart_id: Arc::new(Mutex::new(None)),
            terminal: None,
            direct_door_generation: None,
        }
    }

    /// Signal a live service for restart without waiting for it to exit.
    pub(crate) fn request_restart(
        &mut self,
        journal: &Path,
    ) -> Result<RestartRequestOutcome, nix::errno::Errno> {
        if self.restart_requested {
            return Ok(RestartRequestOutcome::Ignored);
        }
        if let Some(process) = self.process.as_ref() {
            let pid = process.pid();
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            )?;
            self.restart_requested = true;
            return Ok(RestartRequestOutcome::Signaled { pid });
        }
        if self.terminal.is_some() {
            clear_failed_record(journal, self.service);
            self.terminal = None;
            self.restart_policy.reset_unsuccessful_starts();
            self.restart_at = Some(Instant::now());
            return Ok(RestartRequestOutcome::Revived);
        }
        Ok(RestartRequestOutcome::Ignored)
    }

    pub(crate) fn record_exit(&mut self, exit: AppExit) -> RestartDecision {
        let uptime = self
            .started_at
            .take()
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        self.process = None;
        self.restart_requested = false;
        let (policy_code, reason, exit_code) = match exit {
            AppExit::Process { code } => (code, describe_exit(code), Some(code)),
            AppExit::SpawnFailure => (-1, "failed to spawn process".to_owned(), None),
        };
        match self.restart_policy.decide_after_exit(policy_code, uptime) {
            RestartDecision::Retry(delay) => {
                let delay = if self.fast_fixture_timing {
                    delay.min(Duration::from_millis(10))
                } else {
                    delay
                };
                self.restart_at = Some(Instant::now() + delay);
                self.terminal = None;
                RestartDecision::Retry(delay)
            }
            RestartDecision::GiveUp => {
                self.restart_at = None;
                self.terminal = Some(TerminalState {
                    reason,
                    exit_code,
                    restart_attempts: u32::try_from(self.restart_policy.unsuccessful_starts())
                        .unwrap_or(u32::MAX),
                });
                RestartDecision::GiveUp
            }
        }
    }

    pub(crate) fn crashed_candidate(&self) -> Option<CrashedServiceCandidate> {
        let terminal = self.terminal.as_ref()?;
        Some(CrashedServiceCandidate {
            name: self.service.as_str().to_owned(),
            restart_attempts: terminal.restart_attempts,
            phase: RuntimePhase::Failed,
            reason_code: Some(ReasonCode::from_wire(terminal.reason.clone())),
        })
    }
}

fn failed_path(journal: &Path, service: AppService) -> PathBuf {
    journal
        .join("health")
        .join(format!("{}.failed", service.as_str()))
}

fn write_failed_record(journal: &Path, app: &ManagedAppProcess) {
    let Some(terminal) = app.terminal.as_ref() else {
        return;
    };
    if let Err(error) = write_json(
        failed_path(journal, app.service),
        &FailedRecord {
            exit_code: terminal.exit_code,
            restart_attempts: terminal.restart_attempts,
            reason: terminal.reason.clone(),
        },
        JsonWriteOptions::default(),
    ) {
        eprintln!(
            "supervisor: failed to write {}.failed: {error}",
            app.service.as_str()
        );
    }
}

fn clear_failed_record(journal: &Path, service: AppService) {
    let _ = std::fs::remove_file(failed_path(journal, service));
}

fn selected_direct_door_port(journal: &Path, requested: Option<u16>) -> Result<u16, String> {
    requested
        .map(Ok)
        .unwrap_or_else(|| read_direct_door_port(journal).map_err(|error| error.to_string()))
}

pub(crate) fn apply_app_exit(app: &mut ManagedAppProcess, journal: &Path, exit: AppExit) {
    if app.service == AppService::Convey
        && let Some(generation) = app.direct_door_generation.take()
    {
        match read_direct_door_port(journal) {
            Ok(port) => {
                if let Err(error) = withhold_direct_door(journal, generation, port) {
                    eprintln!("supervisor: failed to withhold direct-door record: {error}");
                }
            }
            Err(error) => {
                eprintln!("supervisor: failed to read direct-door port while withholding: {error}");
            }
        }
    }
    if matches!(app.record_exit(exit), RestartDecision::GiveUp) {
        write_failed_record(journal, app);
    }
}

fn fixture_marker_path(journal: &Path, service: AppService) -> String {
    journal
        .join("health")
        .join(format!("fixture-{}.marker", service.as_str()))
        .display()
        .to_string()
}

fn fixture_argv(service: AppService, binary: &str, journal: &Path) -> Vec<String> {
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
    pub sync_conflict: bool,
}

impl SupervisorState {
    pub(crate) fn into_shutdown_driver<'a>(
        self,
        runtime: &'a tokio::runtime::Runtime,
    ) -> SupervisorShutdownDriver<'a> {
        SupervisorShutdownDriver {
            state: self,
            runtime,
        }
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
    [
        (AppService::Convey, !remote && !options.no_convey),
        (AppService::Sense, !remote),
        (AppService::Cortex, !remote && !options.no_cortex),
        (AppService::Spl, !remote && !options.no_spl),
    ]
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

pub(crate) fn spawn_app_process(
    app: &mut ManagedAppProcess,
    journal: &Path,
    sink: Arc<CallosumSocketServer>,
) -> Result<(), SpawnError> {
    if app.service == AppService::Convey {
        match peek_direct_door_generation(journal) {
            Ok(generation) => app.direct_door_generation = Some(generation),
            Err(error) => {
                eprintln!("supervisor: failed to peek direct-door generation: {error}");
                app.direct_door_generation = None;
            }
        }
    }
    let process = ManagedProcess::spawn(
        app.argv.clone(),
        SpawnOptions {
            journal_root: journal.to_path_buf(),
            reference: format!("supervisor-app-{}", app.service.as_str()),
            day: None,
            sink: Some(Arc::new(SupervisorProcessSink {
                server: sink,
                restart_id: Arc::clone(&app.restart_id),
            })),
            environment: BTreeMap::from([(
                OsString::from("SOL_SUPERVISOR_SPAWNED"),
                OsString::from("1"),
            )]),
        },
    )?;
    app.process = Some(process);
    app.started_at = Some(Instant::now());
    app.restart_at = None;
    app.restart_requested = false;
    Ok(())
}

fn start_app_process(app: &mut ManagedAppProcess, journal: &Path, sink: Arc<CallosumSocketServer>) {
    clear_failed_record(journal, app.service);
    if let Err(error) = spawn_app_process(app, journal, sink) {
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
) -> bool {
    let start = Instant::now();
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
                    return false;
                }
            },
            None => return false,
        };
        if let Some(exit_code) = exited {
            apply_app_exit(app, journal, AppExit::Process { code: exit_code });
            eprintln!(
                "supervisor: convey exited during startup (exit {exit_code}); continuing into supervise loop"
            );
            return false;
        }
        if probe.is_ready(journal, &app.argv) {
            return true;
        }
        if start.elapsed() >= probe.wait_window() {
            eprintln!(
                "supervisor: convey was not ready during startup; continuing into supervise loop"
            );
            return false;
        }
        tokio::time::sleep(probe.poll_interval()).await;
    }
}

async fn start_app_stack(
    app_processes: &mut [ManagedAppProcess],
    journal: &Path,
    sink: Arc<CallosumSocketServer>,
    probe: &dyn ConveyReadinessProbe,
) {
    for service in [
        AppService::Convey,
        AppService::Sense,
        AppService::Cortex,
        AppService::Spl,
    ] {
        let app = app_processes
            .iter_mut()
            .find(|app| app.service == service)
            .expect("app process inventory is complete");
        if !app.enabled {
            continue;
        }
        start_app_process(app, journal, sink.clone());
        if service == AppService::Convey && app.process.is_some() {
            let _ = wait_for_convey_ready(app, journal, probe).await;
        }
    }
}

pub(crate) async fn boot_and_tick(
    lifecycle: SupervisorLifecycle,
    journal: PathBuf,
    options: SupervisorOptions,
) -> Result<SupervisorOutcome, String> {
    let mut shutdown_signals = tick::ShutdownSignals::install()?;
    let server = Arc::new(
        CallosumSocketServer::bind(journal.join("health/callosum.sock"))
            .await
            .map_err(|error| error.to_string())?,
    );
    let mut connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), serde_json::Map::new());
    connection.start();
    wait_for_callosum_connection(&mut connection).await?;
    let default_cap = std::env::var("SOLSTONE_SUPERVISOR_TASK_CAP_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TASK_MAX_RUNTIME);
    let queue = TaskQueue::new(TaskQueueOptions {
        journal_root: journal.clone(),
        cap_resolver: Arc::new(DefaultCapResolver::new(default_cap)),
        process_state_probe: Arc::new(SystemProcessStateProbe),
        queue_sink: Some(Arc::new(SupervisorTaskQueueSink(Arc::clone(&server)))),
        process_sink: Some(Arc::new(SupervisorProcessSink {
            server: Arc::clone(&server),
            restart_id: Arc::new(Mutex::new(None)),
        })),
        ready: true,
        before_deadline_commit: None,
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
    let parakeet_shared = Arc::new(ParakeetRuntimeShared::default());
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
        let mut scheduler = ScheduleEngine::init(
            journal.join("config/schedules.json"),
            journal.join("health/scheduler.json"),
            now,
        )
        .map_err(|error| error.to_string())?
        .0;
        let schedule_sink = SupervisorScheduleSink {
            queue: queue.clone(),
            server: server.clone(),
        };
        let _ = scheduler.catch_up(now, &schedule_sink);
        Some(scheduler)
    };
    let fixture_binary = app_fixture_binary();
    let journal_binary = if fixture_binary.is_none() && !remote {
        match resolve_journal_binary() {
            Ok(binary) => Some(binary),
            Err(error) => {
                eprintln!("supervisor: failed to resolve journal binary: {error}");
                None
            }
        }
    } else {
        None
    };
    let fast_fixture_timing = fixture_binary.is_some()
        && std::env::var(APP_FIXTURE_FAST_TIMING_ENV).as_deref() == Ok("1");
    let readiness_probe = convey_readiness_probe(fixture_binary.as_deref(), fast_fixture_timing);
    let convey_port = if options.port != 0 {
        options.port
    } else {
        resolve_available_port().map_err(|error| error.to_string())?
    };
    let direct_port = selected_direct_door_port(&journal, options.direct_port)?;
    persist_direct_door_port(&journal, direct_port).map_err(|error| error.to_string())?;
    initialize_direct_door(&journal, direct_port).map_err(|error| error.to_string())?;
    let mut app_processes = app_processes(
        &options,
        &journal,
        fixture_binary.as_deref(),
        journal_binary.as_deref(),
        convey_port,
        fast_fixture_timing,
    );
    start_app_stack(
        &mut app_processes,
        &journal,
        server.clone(),
        readiness_probe.as_ref(),
    )
    .await;
    lifecycle
        .signal_ready(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |value| value.as_secs_f64()),
            serde_json::Map::new(),
        )
        .map_err(|error| error.to_string())?;
    let heartbeat_filename = lifecycle.heartbeat_filename().to_owned();
    let mut state = SupervisorState {
        journal,
        is_remote_mode: remote,
        no_daily: options.no_daily,
        server,
        connection,
        queue,
        last_sync_snapshot: None,
        heartbeat_filename,
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
        // The first supervise tick may immediately consider persisted retry
        // watermarks. A subsequent catchup drain resets this watermark.
        last_retry_expiry_drain: Instant::now() - tick::RETRY_EXPIRY_INTERVAL,
        wedge: WedgeState::default(),
        timing: SupervisorTiming::for_app_fixture(fast_fixture_timing),
    };
    let sync_conflict = tick::run(&mut state, &mut shutdown_signals).await;
    Ok(SupervisorOutcome {
        lifecycle,
        state,
        regime: ShutdownRegime::Standard,
        sync_conflict,
    })
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
    use super::{resolve_journal_binary_from, selected_direct_door_port};
    use std::path::{Path, PathBuf};

    #[test]
    fn resolves_journal_binary_from_executable_directory() {
        assert_eq!(
            resolve_journal_binary_from(Path::new("/foo/bar")),
            PathBuf::from("/foo/bar/solstone-core-journal")
        );
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
}
