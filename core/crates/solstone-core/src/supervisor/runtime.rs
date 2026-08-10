// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_cli::SupervisorOptions;
use solstone_core_local::plan::Platform;
use solstone_core_system::cap::{DEFAULT_TASK_MAX_RUNTIME, DefaultCapResolver};
use solstone_core_system::lifecycle::{ShutdownRegime, SupervisorLifecycle, SyncSnapshot};
use solstone_core_system::process::{ManagedProcess, RestartPolicy, SpawnError, SpawnOptions};
use solstone_core_system::provider_runtime::{
    FileRuntimeStore, LocalLifecycleSeam, LocalProbeSeam, LocalRuntimeShared, LocalTruthConfig,
    LocalTruthSeam, NoopWorkers, ParakeetLaunchConfig, ParakeetLifecycleSeam, ParakeetPlacement,
    ParakeetProbeSeam, ParakeetRuntimeShared, ProviderName, ProviderRuntimeCoordinator,
    ProviderRuntimeState, SystemRuntimeClock,
};
use solstone_core_system::queue::{SystemProcessStateProbe, TaskQueue, TaskQueueOptions};
use solstone_core_system::schedule::{ScheduleEngine, ScheduleNow};

use super::bus::{SupervisorProcessSink, SupervisorScheduleSink, SupervisorTaskQueueSink};
use super::shutdown::SupervisorShutdownDriver;
use super::tick;

const APP_FIXTURE_ENABLED_ENV: &str = "SOLSTONE_SUPERVISOR_APP_FIXTURE";
const APP_FIXTURE_BINARY_ENV: &str = "SOLSTONE_SUPERVISOR_APP_BINARY";
/// Fixture Convey argv override; test-constructed paths must not contain spaces.
const APP_FIXTURE_CONVEY_ARGV_ENV: &str = "SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV";
const CONVEY_READY_WINDOW: Duration = Duration::from_secs(60);
const CONVEY_READY_INTERVAL: Duration = Duration::from_millis(100);
const CONVEY_READY_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
const FIXTURE_CONVEY_READY_WINDOW: Duration = Duration::from_secs(3);
const FIXTURE_CONVEY_READY_INTERVAL: Duration = Duration::from_millis(20);

pub(crate) struct SupervisorState {
    pub journal: PathBuf,
    pub is_remote_mode: bool,
    pub server: Arc<CallosumSocketServer>,
    pub connection: CallosumSocketConnection,
    pub queue: TaskQueue,
    pub last_sync_snapshot: Option<SyncSnapshot>,
    pub heartbeat_filename: String,
    pub stale_heartbeats: Vec<super::status::StaleHeartbeatStatus>,
    pub shutdown_started: AtomicBool,
    pub started: Instant,
    pub scheduler: ScheduleEngine,
    pub recorded_schedule_completions: BTreeSet<String>,
    pub app_processes: Vec<ManagedAppProcess>,
    pub local: LocalProvider,
    pub parakeet: ParakeetProvider,
    pub flush: FlushState,
    pub daily: DailyState,
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

    fn production_argv(self, journal_binary: &Path) -> Vec<String> {
        let mut argv = vec![journal_binary.display().to_string()];
        argv.extend(
            match self {
                Self::Convey => ["convey", "--port", "5015"].as_slice(),
                Self::Sense => ["sense"].as_slice(),
                Self::Cortex => ["cortex"].as_slice(),
                Self::Spl => ["spl"].as_slice(),
            }
            .iter()
            .map(|value| (*value).to_owned()),
        );
        argv
    }
}

pub(crate) struct ManagedAppProcess {
    pub service: AppService,
    pub enabled: bool,
    pub argv: Vec<String>,
    pub process: Option<ManagedProcess>,
    pub started_at: Option<Instant>,
    pub restart_policy: RestartPolicy,
    pub restart_at: Option<Instant>,
}

impl ManagedAppProcess {
    fn new(
        service: AppService,
        enabled: bool,
        journal: &Path,
        fixture_binary: Option<&str>,
        journal_binary: Option<&Path>,
    ) -> Self {
        let argv = fixture_binary.map_or_else(
            || journal_binary.map_or_else(Vec::new, |binary| service.production_argv(binary)),
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
        }
    }

    pub(crate) fn record_exit(&mut self, exit_code: i32) {
        let uptime = self
            .started_at
            .take()
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        self.process = None;
        let delay = self.restart_policy.delay_after_exit(exit_code, uptime);
        self.restart_at = Some(Instant::now() + delay);
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
        "continuous-lines".to_owned(),
        fixture_marker_path(journal, service),
    ]);
    argv
}

fn ready_sleep_marker_path(argv: &[String]) -> Option<&str> {
    (argv.get(1).map(String::as_str) == Some("ready-sleep"))
        .then(|| argv.get(2).map(String::as_str))
        .flatten()
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

struct FixtureConveyReadinessProbe;

impl ConveyReadinessProbe for FixtureConveyReadinessProbe {
    fn is_ready(&self, _: &Path, convey_argv: &[String]) -> bool {
        ready_sleep_marker_path(convey_argv).is_some_and(|path| Path::new(path).exists())
    }

    fn wait_window(&self) -> Duration {
        FIXTURE_CONVEY_READY_WINDOW
    }

    fn poll_interval(&self) -> Duration {
        FIXTURE_CONVEY_READY_INTERVAL
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
    pub truth: NoopWorkers,
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

fn convey_readiness_probe(fixture_binary: Option<&str>) -> Box<dyn ConveyReadinessProbe> {
    if fixture_binary.is_some() {
        return Box::new(FixtureConveyReadinessProbe);
    }
    Box::new(TcpConveyReadinessProbe)
}

fn app_processes(
    options: &SupervisorOptions,
    journal: &Path,
    fixture_binary: Option<&str>,
    journal_binary: Option<&Path>,
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
        ManagedAppProcess::new(service, enabled, journal, fixture_binary, journal_binary)
    })
    .collect()
}

pub(crate) fn spawn_app_process(
    app: &mut ManagedAppProcess,
    journal: &Path,
    sink: Arc<CallosumSocketServer>,
) -> Result<(), SpawnError> {
    let process = ManagedProcess::spawn(
        app.argv.clone(),
        SpawnOptions {
            journal_root: journal.to_path_buf(),
            reference: format!("supervisor-app-{}", app.service.as_str()),
            day: None,
            sink: Some(Arc::new(SupervisorProcessSink(sink))),
            environment: BTreeMap::from([(
                OsString::from("SOL_SUPERVISOR_SPAWNED"),
                OsString::from("1"),
            )]),
        },
    )?;
    app.process = Some(process);
    app.started_at = Some(Instant::now());
    app.restart_at = None;
    Ok(())
}

fn start_app_process(app: &mut ManagedAppProcess, journal: &Path, sink: Arc<CallosumSocketServer>) {
    if let Err(error) = spawn_app_process(app, journal, sink) {
        eprintln!(
            "supervisor: failed to start {}: {error}",
            app.service.as_str()
        );
        app.record_exit(-1);
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
            app.record_exit(exit_code);
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
    let server = Arc::new(
        CallosumSocketServer::bind(journal.join("health/callosum.sock"))
            .await
            .map_err(|error| error.to_string())?,
    );
    let mut connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), serde_json::Map::new());
    connection.start();
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
        process_sink: Some(Arc::new(SupervisorProcessSink(Arc::clone(&server)))),
        ready: true,
        before_deadline_commit: None,
    });
    let clock: Arc<dyn solstone_core_system::provider_runtime::RuntimeClock> =
        Arc::new(SystemRuntimeClock::default());
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
                // The fixture path deliberately uses the lightweight MLX artifact
                // proof instead of inspecting host NVIDIA hardware. Its launch
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
    let parakeet_shared = Arc::new(ParakeetRuntimeShared::default());
    let fingerprint = "native-parakeet-seeded".to_owned();
    parakeet_shared.record_launch_request(
        Some(fingerprint.clone()),
        ParakeetLaunchConfig {
            binary_backend: "cpu".to_owned(),
            env_updates: Default::default(),
            gpu_index: None,
            binary_path: std::env::var_os("SOLSTONE_PARAKEET_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("parakeet-server")),
            model_path: std::env::var_os("SOLSTONE_PARAKEET_MODEL")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("parakeet")),
            threads: 4,
            desired_fingerprint_json: "{}".to_owned(),
            desired_fingerprint_sha256: fingerprint.clone(),
            placement: ParakeetPlacement::Cpu,
        },
    );
    let mut parakeet_state = ProviderRuntimeState::new(ProviderName::Parakeet);
    parakeet_state.desired_fingerprint = Some(fingerprint);
    parakeet_state.has_plan = true;
    parakeet_state.latest_phase = solstone_core_system::provider_runtime::RuntimePhase::Starting;
    // No ParakeetTruthSeam exists yet; never dispatch the NoopWorkers truth seam.
    parakeet_state.next_truth_at = f64::MAX;
    parakeet_state.next_probe_at = 100.0;
    let parakeet = ParakeetProvider {
        coordinator: ProviderRuntimeCoordinator::new(),
        shared: parakeet_shared.clone(),
        truth: NoopWorkers,
        lifecycle: ParakeetLifecycleSeam::new(parakeet_shared.clone(), clock.clone()),
        probe: ParakeetProbeSeam::new(parakeet_shared.clone(), journal.clone()),
        store: FileRuntimeStore::new(
            journal.clone(),
            ProviderName::Parakeet,
            parakeet_shared.clone(),
            clock,
        ),
        state: parakeet_state,
        processes: Vec::new(),
    };
    let wall = chrono::Local::now();
    let now = ScheduleNow {
        local: wall.naive_local(),
        unix_millis: wall.timestamp_millis(),
    };
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
    let fixture_binary = app_fixture_binary();
    let remote = options.remote.as_deref().is_some_and(|url| !url.is_empty());
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
    let readiness_probe = convey_readiness_probe(fixture_binary.as_deref());
    let mut app_processes = app_processes(
        &options,
        &journal,
        fixture_binary.as_deref(),
        journal_binary.as_deref(),
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
    };
    let sync_conflict = tick::run(&mut state).await;
    Ok(SupervisorOutcome {
        lifecycle,
        state,
        regime: ShutdownRegime::Standard,
        sync_conflict,
    })
}

#[cfg(test)]
mod tests {
    use super::resolve_journal_binary_from;
    use std::path::{Path, PathBuf};

    #[test]
    fn resolves_journal_binary_from_executable_directory() {
        assert_eq!(
            resolve_journal_binary_from(Path::new("/foo/bar")),
            PathBuf::from("/foo/bar/solstone-core-journal")
        );
    }
}
