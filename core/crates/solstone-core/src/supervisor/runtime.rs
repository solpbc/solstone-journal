// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
use solstone_core_local::plan::Platform;
use solstone_core_system::cap::{DEFAULT_TASK_MAX_RUNTIME, DefaultCapResolver};
use solstone_core_system::lifecycle::{ShutdownRegime, SupervisorLifecycle, SyncSnapshot};
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

pub(crate) struct SupervisorState {
    pub journal: PathBuf,
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
    pub local: LocalProvider,
    pub parakeet: ParakeetProvider,
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
    }
}

pub(crate) async fn boot_and_tick(
    lifecycle: SupervisorLifecycle,
    journal: PathBuf,
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
        local,
        parakeet,
    };
    let sync_conflict = tick::run(&mut state).await;
    Ok(SupervisorOutcome {
        lifecycle,
        state,
        regime: ShutdownRegime::Standard,
        sync_conflict,
    })
}
