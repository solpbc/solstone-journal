// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use solstone_core_callosum::CallosumEnvelope;
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::day_path;
use solstone_core_local::nvidia::{ArtifactTrust, NvidiaProbe};
use solstone_core_local::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_system::lifecycle::{
    DEFAULT_INTERVAL_SECONDS, check_sync, machine_id, sync_conflict_event, write_sync_heartbeat,
};
use solstone_core_system::process::{
    ProcessObservation as SystemProcessObservation, ProcessObservationTuple,
    classify_process_observation,
};
use solstone_core_system::provider_runtime::{
    CortexEventKind, CortexOutcomeEvent, LocalLaunchCommon, LocalLaunchConfig, ProbeStatus,
    ProviderName, ProviderRetryState, ProviderRuntimeEvent, ProviderRuntimeEventSink,
    ProviderRuntimeNow, ProviderRuntimeState, ReasonCode, ReconcileContext, RuntimePhase,
    RuntimeStore, RuntimeStoreError, cancel_start, store_error_phase,
};
use solstone_core_system::request::{BusTaskRequest, ExecutionRequest, TaskArgv};
use solstone_core_system::schedule::{ScheduleNow, ScheduleStatus};
use solstone_core_system::status_wire::{
    CrashedServiceCandidate, ProcessObservation as WireProcessObservation, ServiceCandidate,
    StaleHeartbeatWireInput, SupervisorStatusWireInput, project_supervisor_status,
};
use solstone_core_system::{
    catchup::{CatchupError, eligible_catchup_days},
    queue::{SubmitOutcome, TaskQueue, TaskQueueStatusSnapshot},
};

use super::bus::{SupervisorProviderSink, SupervisorScheduleSink, emit};
use super::config::{no_thinking_engine_chosen, processing_is_deferred};
use super::runtime::{
    AppExit, AppService, DailyState, FlushState, ManagedAppProcess, RestartRequestOutcome,
    SupervisorState, apply_app_exit,
};

const MAX_INBOUND_PER_TICK: usize = 256;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3600);

struct AppProcessSample {
    service: AppService,
    process_count: usize,
    tuple: Option<ProcessObservationTuple<i32>>,
}

enum StatusEmissionPlan {
    Errors(Vec<&'static str>),
    Status(SupervisorStatusWireInput),
}

pub(crate) struct ShutdownSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl ShutdownSignals {
    pub(crate) fn install() -> Result<Self, String> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                terminate: signal(SignalKind::terminate()).map_err(|error| error.to_string())?,
                interrupt: signal(SignalKind::interrupt()).map_err(|error| error.to_string())?,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    async fn wait(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.terminate.recv() => {},
            _ = self.interrupt.recv() => {},
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    }
}

struct StatusEmissionInputs<'a> {
    app_observations: Vec<(AppService, SystemProcessObservation)>,
    app_crashed: Vec<CrashedServiceCandidate>,
    local_observation: SystemProcessObservation,
    parakeet_observation: SystemProcessObservation,
    local_state: &'a ProviderRuntimeState,
    parakeet_state: &'a ProviderRuntimeState,
    supervisor_pid: u32,
    supervisor_uptime_seconds: u64,
    queue: TaskQueueStatusSnapshot,
    stale_heartbeats: Vec<StaleHeartbeatWireInput>,
    schedules: Vec<ScheduleStatus>,
    callosum_clients: usize,
}

fn plan_status_emission(inputs: StatusEmissionInputs<'_>) -> StatusEmissionPlan {
    let mut indeterminate_services = inputs
        .app_observations
        .iter()
        .filter_map(|(service, observation)| {
            matches!(observation, SystemProcessObservation::Indeterminate)
                .then_some(service.as_str())
        })
        .collect::<Vec<_>>();
    if matches!(
        &inputs.local_observation,
        SystemProcessObservation::Indeterminate
    ) {
        indeterminate_services.push(ProviderName::Local.as_str());
    }
    if matches!(
        &inputs.parakeet_observation,
        SystemProcessObservation::Indeterminate
    ) {
        indeterminate_services.push(ProviderName::Parakeet.as_str());
    }
    if !indeterminate_services.is_empty() {
        return StatusEmissionPlan::Errors(indeterminate_services);
    }

    let providers = [
        (inputs.local_state, inputs.local_observation),
        (inputs.parakeet_state, inputs.parakeet_observation),
    ];
    let mut services = vec![ServiceCandidate::SupervisorSelf {
        reference: "supervisor".into(),
        pid: inputs.supervisor_pid,
        uptime_seconds: inputs.supervisor_uptime_seconds,
    }];
    services.extend(
        inputs
            .app_observations
            .into_iter()
            .map(|(service, observation)| ServiceCandidate::App {
                name: service.as_str().to_owned(),
                observation: wire_observation(observation),
            }),
    );
    services.extend(
        providers
            .iter()
            .map(|(provider, observation)| ServiceCandidate::Provider {
                provider: provider.provider,
                observation: wire_observation(observation.clone()),
                phase: provider.latest_phase,
                reason_code: provider.latest_reason_code.clone(),
            }),
    );
    let mut crashed = providers
        .iter()
        .filter(|(provider, _)| is_crashed_phase(provider.latest_phase))
        .map(|(provider, _)| CrashedServiceCandidate {
            name: provider.provider.as_str().to_owned(),
            restart_attempts: provider.retry.attempt_count,
            phase: provider.latest_phase,
            reason_code: provider.latest_reason_code.clone(),
        })
        .collect::<Vec<_>>();
    crashed.extend(inputs.app_crashed);
    StatusEmissionPlan::Status(SupervisorStatusWireInput {
        services,
        crashed,
        queue: inputs.queue,
        stale_heartbeats: inputs.stale_heartbeats,
        schedules: inputs.schedules,
        callosum_clients: inputs.callosum_clients,
    })
}

pub(crate) async fn run(state: &mut SupervisorState, shutdown: &mut ShutdownSignals) -> bool {
    let mut last_status = Instant::now() - state.timing.status_interval;
    let mut last_sync = Instant::now() - Duration::from_secs_f64(DEFAULT_INTERVAL_SECONDS);
    loop {
        let app_samples = reconcile_app_processes(state);
        let tick = Instant::now();
        state.queue.enforce_deadlines(tick);
        record_schedule_completions(state);
        reconcile_providers(state);
        drain_inbound(state).await;
        let wall = chrono::Local::now();
        let wall_now = SystemTime::now();
        check_segment_flush(
            &state.journal,
            &state.queue,
            state.is_remote_mode,
            &mut state.flush,
            false,
            tick,
        );
        if !state.no_daily
            && let Err(error) = handle_daily_tasks(
                &state.journal,
                &state.queue,
                state.is_remote_mode,
                &mut state.daily,
                &mut state.flush,
                wall.date_naive(),
                wall_now,
            )
        {
            eprintln!("supervisor: daily catchup drain failed: {error}");
        }
        if let Some(scheduler) = state.scheduler.as_mut() {
            let schedule_sink = SupervisorScheduleSink {
                queue: state.queue.clone(),
                server: state.server.clone(),
            };
            let _ = scheduler.check(
                ScheduleNow {
                    local: wall.naive_local(),
                    unix_millis: wall.timestamp_millis(),
                },
                &schedule_sink,
            );
        }
        if last_sync.elapsed().as_secs_f64() >= DEFAULT_INTERVAL_SECONDS {
            if sync_tick(state) {
                return true;
            }
            last_sync = Instant::now();
        }
        if last_status.elapsed() >= state.timing.status_interval {
            let status_now = Instant::now();
            let app_observations = app_samples
                .into_iter()
                .map(|sample| (sample.service, observe_app_process(sample, status_now)))
                .collect::<Vec<_>>();
            let local_observation = state
                .local
                .shared
                .observe_current_process(&state.local.processes, status_now);
            let parakeet_observation = state
                .parakeet
                .shared
                .observe_current_process(&state.parakeet.processes, status_now);
            let queue = state.queue.collect_status_snapshot(status_now);
            let wall = chrono::Local::now();
            let schedules = state
                .scheduler
                .as_ref()
                .map(|scheduler| {
                    scheduler.collect_status(ScheduleNow {
                        local: wall.naive_local(),
                        unix_millis: wall.timestamp_millis(),
                    })
                })
                .unwrap_or_default();
            match plan_status_emission(StatusEmissionInputs {
                app_observations,
                app_crashed: state
                    .app_processes
                    .iter()
                    .filter_map(ManagedAppProcess::crashed_candidate)
                    .collect(),
                local_observation,
                parakeet_observation,
                local_state: &state.local.state,
                parakeet_state: &state.parakeet.state,
                supervisor_pid: std::process::id(),
                supervisor_uptime_seconds: status_now
                    .saturating_duration_since(state.started)
                    .as_secs(),
                queue,
                stale_heartbeats: state
                    .stale_heartbeats
                    .iter()
                    .map(stale_heartbeat_wire_input)
                    .collect(),
                schedules,
                callosum_clients: state.server.client_count(),
            }) {
                StatusEmissionPlan::Errors(services) => {
                    for service in services {
                        emit(
                            &state.server,
                            "supervisor",
                            "status-error",
                            Map::from_iter([
                                ("service".into(), json!(service)),
                                ("reason".into(), json!("process-observation-failed")),
                            ]),
                        );
                    }
                }
                StatusEmissionPlan::Status(input) => {
                    emit(
                        &state.server,
                        "supervisor",
                        "status",
                        project_supervisor_status(input),
                    );
                }
            }
            last_status = status_now;
        }
        tokio::select! {
            _ = tokio::time::sleep(state.timing.tick_interval) => {},
            _ = shutdown.wait() => return false,
        }
    }
}

/// Flush the last live segment after it has been idle for the Python-compatible timeout.
pub(crate) fn check_segment_flush(
    journal: &Path,
    queue: &TaskQueue,
    is_remote: bool,
    flush: &mut FlushState,
    force: bool,
    now: Instant,
) {
    if is_remote
        || flush.last_segment_ts.is_none()
        || flush.flushed
        || processing_is_deferred(journal)
        || no_thinking_engine_chosen(journal)
        || (!force
            && flush.last_segment_ts.is_some_and(|last_segment_ts| {
                now.saturating_duration_since(last_segment_ts) < FLUSH_TIMEOUT
            }))
    {
        return;
    }
    let (Some(day), Some(segment)) = (flush.day.as_deref(), flush.segment.as_deref()) else {
        return;
    };

    flush.flushed = true;
    let _ = submit_think(
        queue,
        flush_think_argv(day, segment, flush.stream.as_deref()),
        day,
        format!("supervisor-flush-{day}-{segment}"),
    );
}

/// Handle one detected local-day rollover, including a forced previous-day flush.
pub(crate) fn handle_daily_tasks(
    journal: &Path,
    queue: &TaskQueue,
    is_remote: bool,
    daily: &mut DailyState,
    flush: &mut FlushState,
    today: chrono::NaiveDate,
    now: SystemTime,
) -> Result<(), CatchupError> {
    if is_remote || daily.last_day == Some(today) {
        return Ok(());
    }
    let Some(previous_day) = daily.last_day else {
        eprintln!("supervisor: daily state not initialized; skipping daily processing");
        daily.last_day = Some(today);
        return Ok(());
    };

    daily.last_day = Some(today);
    let previous_day = previous_day.format("%Y%m%d").to_string();
    if !flush.flushed && flush.day.as_deref() == Some(previous_day.as_str()) {
        let tick = flush.last_segment_ts.unwrap_or_else(Instant::now);
        check_segment_flush(journal, queue, is_remote, flush, true, tick);
    }
    run_catchup_drain(
        journal,
        queue,
        &BTreeSet::from([today.format("%Y%m%d").to_string()]),
        &[],
        now,
    )
}

/// Submit one daily think task for each selected, eligible catchup day.
pub(crate) fn run_catchup_drain(
    journal: &Path,
    queue: &TaskQueue,
    exclude: &BTreeSet<String>,
    force_days: &[String],
    now: SystemTime,
) -> Result<(), CatchupError> {
    if no_thinking_engine_chosen(journal) {
        return Ok(());
    }
    for day in eligible_catchup_days(journal, force_days, exclude, now)? {
        let _ = submit_think(
            queue,
            daily_think_argv(&day),
            &day,
            format!("supervisor-catchup-{day}"),
        );
    }
    Ok(())
}

fn flush_think_argv(day: &str, segment: &str, stream: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "journal".to_owned(),
        "think".to_owned(),
        "-v".to_owned(),
        "--day".to_owned(),
        day.to_owned(),
        "--segment".to_owned(),
        segment.to_owned(),
        "--flush".to_owned(),
    ];
    if let Some(stream) = stream {
        argv.extend(["--stream".to_owned(), stream.to_owned()]);
    }
    argv
}

fn daily_think_argv(day: &str) -> Vec<String> {
    vec![
        "journal".to_owned(),
        "think".to_owned(),
        "-v".to_owned(),
        "--day".to_owned(),
        day.to_owned(),
    ]
}

fn submit_think(
    queue: &TaskQueue,
    argv: Vec<String>,
    day: &str,
    reference: String,
) -> solstone_core_system::queue::SubmitOutcome {
    submit_task(queue, argv, reference, Some(day))
}

fn submit_task(
    queue: &TaskQueue,
    argv: Vec<String>,
    reference: String,
    day: Option<&str>,
) -> solstone_core_system::queue::SubmitOutcome {
    let cmd = TaskArgv::from_wire(argv).expect("supervisor constructs a non-empty think argv");
    queue.submit(ExecutionRequest::Bus(BusTaskRequest {
        cmd,
        reference,
        day: day.map(str::to_owned),
        scheduler_name: None,
        queue_if_active_cmd_differs: false,
    }))
}

fn reconcile_app_processes(state: &mut SupervisorState) -> Vec<AppProcessSample> {
    let journal = state.journal.clone();
    let server = state.server.clone();
    let mut samples = Vec::new();
    for app in &mut state.app_processes {
        if !app.enabled {
            continue;
        }
        if let Some(process) = app.process.as_mut() {
            let reference = format!("supervisor-app-{}", app.service.as_str());
            let pid = process.pid();
            let started_at = app.started_at;
            let poll = process.poll();
            match &poll {
                Ok(Some(exit_code)) => {
                    process.cleanup();
                    eprintln!(
                        "supervisor: {} exited with {}; scheduling restart",
                        app.service.as_str(),
                        exit_code
                    );
                    apply_app_exit(app, &journal, AppExit::Process { code: *exit_code });
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!(
                        "supervisor: failed to poll {}: {error}",
                        app.service.as_str()
                    );
                }
            }
            let sample = AppProcessSample {
                service: app.service,
                process_count: 1,
                tuple: started_at.map(|started_at| ProcessObservationTuple {
                    reference,
                    pid,
                    started_at,
                    poll,
                }),
            };
            samples.push(sample);
            continue;
        }
        if app
            .restart_at
            .is_some_and(|restart_at| Instant::now() >= restart_at)
            && let Err(error) = super::runtime::spawn_app_process(app, &journal, server.clone())
        {
            eprintln!(
                "supervisor: failed to restart {}: {error}",
                app.service.as_str()
            );
            apply_app_exit(app, &journal, AppExit::SpawnFailure);
        }
        samples.push(AppProcessSample {
            service: app.service,
            process_count: usize::from(app.process.is_some()),
            tuple: None,
        });
    }
    samples
}

fn observe_app_process(sample: AppProcessSample, now: Instant) -> SystemProcessObservation {
    classify_process_observation(sample.process_count, false, sample.tuple, now)
}

fn wire_observation(observation: SystemProcessObservation) -> WireProcessObservation {
    match observation {
        SystemProcessObservation::Live {
            reference,
            pid,
            uptime_seconds,
        } => WireProcessObservation::Live {
            reference,
            pid,
            uptime_seconds,
        },
        SystemProcessObservation::ConfirmedAbsent => WireProcessObservation::ConfirmedAbsent,
        SystemProcessObservation::Indeterminate => {
            unreachable!("indeterminate observations are rejected before projection")
        }
    }
}

fn is_crashed_phase(phase: RuntimePhase) -> bool {
    matches!(
        phase,
        RuntimePhase::Failed
            | RuntimePhase::CleanupFailed
            | RuntimePhase::StateCorrupt
            | RuntimePhase::StateUnavailable
    )
}

fn stale_heartbeat_wire_input(
    writer: &solstone_core_system::lifecycle::ForeignWriter,
) -> StaleHeartbeatWireInput {
    StaleHeartbeatWireInput {
        source_filename: writer
            .path
            .file_name()
            .map_or_else(Vec::new, |filename| filename.as_encoded_bytes().to_vec()),
        hostname: writer.hostname.clone(),
        machine_id: writer.machine_id.clone(),
        journal_path: writer.journal_path.clone(),
        pid: writer.pid,
        wall_time: Some(writer.wall_time.clone()),
        malformed: writer.malformed,
    }
}

fn record_schedule_completions(state: &mut SupervisorState) {
    let Some(scheduler) = state.scheduler.as_ref() else {
        return;
    };
    let history = state.queue.history();
    let retained = history
        .iter()
        .map(|record| record.reference.clone())
        .collect::<std::collections::BTreeSet<_>>();
    state
        .recorded_schedule_completions
        .retain(|reference| retained.contains(reference));
    for record in history {
        let Some(name) = record.scheduler_name else {
            continue;
        };
        if !state
            .recorded_schedule_completions
            .insert(record.reference.clone())
        {
            continue;
        }
        let ended_at = record
            .ended_at
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |value| value.as_secs_f64());
        if let Err(error) =
            scheduler.record_completion(&name, ended_at, &record.exit_status, &record.reference)
        {
            eprintln!("supervisor: failed to record schedule completion for {name}: {error}");
        }
    }
}

pub(crate) fn reconcile_providers(state: &mut SupervisorState) {
    let now = ProviderRuntimeNow {
        monotonic_seconds: state.started.elapsed().as_secs_f64(),
    };
    if let Some(in_flight) = state.local.state.truth.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state.local.shared.take_truth_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    record_fixture_local_launch(state);
    if let Some(in_flight) = state.local.state.start.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state.local.shared.take_launch_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    if let Some(in_flight) = state.local.state.stop_cleanup.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state
            .local
            .shared
            .take_stop_cleanup_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    if let Some(in_flight) = state.local.state.probe.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state.local.shared.take_probe_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    let mut local_sink = SupervisorProviderSink(state.server.clone());
    let mut local_context = ReconcileContext {
        truth: &mut state.local.truth,
        lifecycle: &mut state.local.lifecycle,
        probe: &mut state.local.probe,
        store: &mut state.local.store,
        sink: &mut local_sink,
        gate: None,
    };
    state.local.coordinator.reconcile(
        now,
        &mut state.local.state,
        &mut state.local.processes,
        &mut local_context,
    );
    if let Some(in_flight) = state.parakeet.state.truth.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state.parakeet.shared.take_truth_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    if let Some(in_flight) = state.parakeet.state.start.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state.parakeet.shared.take_launch_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    if let Some(in_flight) = state.parakeet.state.stop_cleanup.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state
            .parakeet
            .shared
            .take_stop_cleanup_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    if let Some(in_flight) = state.parakeet.state.probe.as_mut()
        && in_flight.result.is_none()
        && let Some(result) = state.parakeet.shared.take_probe_result(&in_flight.fence)
    {
        in_flight.result = Some(result);
    }
    let mut parakeet_sink = SupervisorProviderSink(state.server.clone());
    let mut parakeet_context = ReconcileContext {
        truth: &mut state.parakeet.truth,
        lifecycle: &mut state.parakeet.lifecycle,
        probe: &mut state.parakeet.probe,
        store: &mut state.parakeet.store,
        sink: &mut parakeet_sink,
        gate: None,
    };
    state.parakeet.coordinator.reconcile(
        now,
        &mut state.parakeet.state,
        &mut state.parakeet.processes,
        &mut parakeet_context,
    );
}

fn record_fixture_local_launch(state: &mut SupervisorState) {
    let Some(fixture) = state.local.fixture_launch.as_ref() else {
        return;
    };
    let Some(fingerprint) = state
        .local
        .state
        .truth
        .as_ref()
        .and_then(|truth| truth.result.as_ref())
        .and_then(|result| result.desired_fingerprint.clone())
    else {
        return;
    };
    if state.local.launch_recorded_for.as_deref() == Some(fingerprint.as_str()) {
        return;
    }
    state.local.shared.record_launch_request(
        Some(fingerprint.clone()),
        LocalLaunchConfig::Cuda {
            common: LocalLaunchCommon {
                desired_fingerprint_json: json!({"provider":"local","stub":true}),
                desired_fingerprint_sha256: fingerprint.clone(),
                model_id: fixture.model_id.clone(),
                model_path: fixture.model_path.clone(),
                mmproj_path: None,
            },
            binary_path: Some(fixture.binary_path.clone()),
            lib_dir: None,
            nvidia_probe: synthetic_nvidia_probe(),
            cuda_embedded_arch_set: vec!["sm_89".to_owned()],
            cuda_min_driver_version: 1,
            cuda_artifact_trust: ArtifactTrust::Trusted,
            cuda_persisted_installed_cuda_target: false,
        },
    );
    state.local.launch_recorded_for = Some(fingerprint);
}

fn synthetic_nvidia_probe() -> NvidiaProbe {
    NvidiaProbe {
        schema: "solstone-local-nvidia-probe-v1".to_owned(),
        detected: true,
        gpu_index: Some(0),
        gpu_name: Some("supervisor fixture GPU".to_owned()),
        compute_cap: Some("8.9".to_owned()),
        arch: Some("sm_89".to_owned()),
        driver_cuda_major: Some(13),
        vram_mib: Some(16_000),
        unified_memory_mib: None,
        probe_error: None,
    }
}

async fn drain_inbound(state: &mut SupervisorState) {
    for _ in 0..MAX_INBOUND_PER_TICK {
        let message =
            match tokio::time::timeout(Duration::ZERO, state.connection.next_message()).await {
                Ok(Some(message)) => message,
                _ => break,
            };
        handle_message(state, message);
    }
}

fn handle_message(state: &mut SupervisorState, message: CallosumEnvelope) {
    handle_supervisor_request(state, &message);
    handle_supervisor_restart(state, &message);
    handle_supervisor_drain(state, &message);
    handle_segment_observed(state, &message);
    handle_activity_recorded(state, &message);
    handle_think_daily_complete(state, &message);
    handle_segment_event_log(state, &message);
    handle_cortex_outcome(state, &message);
}

enum SupervisorRequestError {
    MissingCmd,
    NonStringElement,
    EmptyCmd,
}

impl std::fmt::Display for SupervisorRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::MissingCmd => "request missing cmd array",
            Self::NonStringElement => "request cmd contains a non-string element",
            Self::EmptyCmd => "request cmd is empty",
        };
        write!(formatter, "{msg}")
    }
}

fn decode_supervisor_cmd(message: &CallosumEnvelope) -> Result<TaskArgv, SupervisorRequestError> {
    let Some(Value::Array(command)) = message.extra.get("cmd") else {
        return Err(SupervisorRequestError::MissingCmd);
    };
    let Some(command) = command
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
    else {
        return Err(SupervisorRequestError::NonStringElement);
    };
    TaskArgv::from_wire(command.into_iter().map(str::to_owned).collect())
        .map_err(|_| SupervisorRequestError::EmptyCmd)
}

fn handle_supervisor_request(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "supervisor" || message.event != "request" {
        return;
    }
    let cmd = match decode_supervisor_cmd(message) {
        Ok(cmd) => cmd,
        Err(error) => {
            eprintln!("supervisor: {error}");
            return;
        }
    };
    let request = BusTaskRequest {
        cmd,
        reference: message
            .extra
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or("native-supervisor")
            .to_owned(),
        day: message
            .extra
            .get("day")
            .and_then(Value::as_str)
            .map(str::to_owned),
        scheduler_name: message
            .extra
            .get("scheduler_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        queue_if_active_cmd_differs: false,
    };
    if state.queue.submit(ExecutionRequest::Bus(request)) == SubmitOutcome::Rejected {
        eprintln!("supervisor: request rejected");
    }
}

fn handle_supervisor_restart(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "supervisor" || message.event != "restart" {
        return;
    }
    let Some(service) = message_string(message, "service") else {
        eprintln!("supervisor: restart request missing service");
        return;
    };
    if service == "supervisor" {
        eprintln!("supervisor: refusing self restart request");
        return;
    }
    let Some(app) = state
        .app_processes
        .iter_mut()
        .find(|app| app.service.as_str() == service)
    else {
        eprintln!("supervisor: restart requested for unknown service {service}");
        return;
    };
    let restart_id = message
        .extra
        .get("restart_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match app.request_restart(&state.journal) {
        Ok(RestartRequestOutcome::Signaled { pid }) => {
            *app.restart_id
                .lock()
                .expect("restart correlation lock is not poisoned") = restart_id.clone();
            let mut extra = Map::from_iter([
                ("service".into(), json!(service)),
                ("pid".into(), json!(pid)),
            ]);
            if let Some(restart_id) = restart_id {
                extra.insert("restart_id".into(), json!(restart_id));
            }
            emit(&state.server, "supervisor", "restarting", extra);
        }
        Ok(RestartRequestOutcome::Revived) => {
            *app.restart_id
                .lock()
                .expect("restart correlation lock is not poisoned") = restart_id.clone();
            eprintln!("supervisor: restarting given-up service {service}");
            let mut extra = Map::from_iter([("service".into(), json!(service))]);
            if let Some(restart_id) = restart_id {
                extra.insert("restart_id".into(), json!(restart_id));
            }
            emit(&state.server, "supervisor", "restarting", extra);
        }
        Ok(RestartRequestOutcome::Ignored) => {
            eprintln!("supervisor: restart request ignored for inactive service {service}")
        }
        Err(error) => eprintln!("supervisor: failed to restart {service}: {error}"),
    }
}

fn handle_supervisor_drain(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "supervisor" || message.event != "drain" || state.is_remote_mode {
        return;
    }
    let now = SystemTime::now();
    let result = if let Some(day) = message_string(message, "day") {
        run_catchup_drain(
            &state.journal,
            &state.queue,
            &BTreeSet::new(),
            &[day.to_owned()],
            now,
        )
    } else if message_truthy(message, "exclude_today") {
        run_catchup_drain(
            &state.journal,
            &state.queue,
            &BTreeSet::from([chrono::Local::now().format("%Y%m%d").to_string()]),
            &[],
            now,
        )
    } else {
        run_catchup_drain(&state.journal, &state.queue, &BTreeSet::new(), &[], now)
    };
    if let Err(error) = result {
        eprintln!("supervisor: catchup drain request failed: {error}");
    }
}

fn handle_segment_observed(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "observe" || message.event != "observed" {
        return;
    }
    let Some(segment) = message_string(message, "segment") else {
        eprintln!("supervisor: observed message missing segment");
        return;
    };
    let day = message_string(message, "day")
        .map(str::to_owned)
        .unwrap_or_else(|| chrono::Local::now().format("%Y%m%d").to_string());
    if message_truthy(message, "batch") {
        eprintln!("supervisor: batch observed segment held for daily catchup: {day}/{segment}");
        return;
    }
    if processing_is_deferred(&state.journal) || no_thinking_engine_chosen(&state.journal) {
        eprintln!("supervisor: observed segment held by processing configuration: {day}/{segment}");
        return;
    }
    let stream = message_string(message, "stream").map(str::to_owned);
    state.flush.last_segment_ts = Some(Instant::now());
    state.flush.day = Some(day.clone());
    state.flush.segment = Some(segment.to_owned());
    state.flush.stream = stream.clone();
    state.flush.flushed = false;
    let mut argv = vec![
        "journal".to_owned(),
        "think".to_owned(),
        "-v".to_owned(),
        "--day".to_owned(),
        day.clone(),
        "--segment".to_owned(),
        segment.to_owned(),
    ];
    if let Some(stream) = stream {
        argv.extend(["--stream".to_owned(), stream]);
    }
    argv.push("--live".to_owned());
    let _ = submit_think(
        &state.queue,
        argv,
        &day,
        format!("supervisor-observed-{day}-{segment}"),
    );
}

fn handle_activity_recorded(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "activity" || message.event != "recorded" {
        return;
    }
    let (Some(id), Some(facet), Some(day)) = (
        message_string(message, "id"),
        message_string(message, "facet"),
        message_string(message, "day"),
    ) else {
        eprintln!("supervisor: activity.recorded message missing id, facet, or day");
        return;
    };
    let _ = submit_task(
        &state.queue,
        vec![
            "journal".to_owned(),
            "think".to_owned(),
            "--activity".to_owned(),
            id.to_owned(),
            "--facet".to_owned(),
            facet.to_owned(),
            "--day".to_owned(),
            day.to_owned(),
        ],
        format!("supervisor-activity-{id}"),
        Some(day),
    );
}

fn handle_think_daily_complete(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "think" || message.event != "daily_complete" {
        return;
    }
    let heartbeat_pid = state.journal.join("health/heartbeat.pid");
    if let Ok(contents) = std::fs::read_to_string(&heartbeat_pid)
        && let Ok(pid) = contents.trim().parse::<i32>()
    {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) | Err(nix::errno::Errno::EPERM) => {
                eprintln!("supervisor: heartbeat already running with pid {pid}");
                return;
            }
            Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => eprintln!("supervisor: could not check heartbeat pid {pid}: {error}"),
        }
    }
    let _ = submit_task(
        &state.queue,
        vec!["journal".to_owned(), "heartbeat".to_owned()],
        "supervisor-heartbeat".to_owned(),
        None,
    );
}

fn handle_segment_event_log(state: &SupervisorState, message: &CallosumEnvelope) {
    if !matches!(message.tract.as_str(), "observe" | "think" | "activity") {
        return;
    }
    let (Some(day), Some(segment)) = (
        message_string(message, "day"),
        message_string(message, "segment"),
    ) else {
        return;
    };
    let day_dir = match day_path(&state.journal, Some(day), false) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("supervisor: could not resolve event-log day {day}: {error}");
            return;
        }
    };
    let segment_dir = message_string(message, "stream").map_or_else(
        || day_dir.join(segment),
        |stream| day_dir.join(stream).join(segment),
    );
    if !segment_dir.is_dir() {
        return;
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(segment_dir.join("events.jsonl"))?;
        serde_json::to_writer(&mut file, message).map_err(std::io::Error::other)?;
        file.write_all(b"\n")
    })();
    if let Err(error) = result {
        eprintln!("supervisor: failed to append segment event log: {error}");
    }
}

fn handle_cortex_outcome(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "cortex"
        || !matches!(message.event.as_str(), "start" | "finish" | "error")
        || state.is_remote_mode
    {
        return;
    }
    let Some(use_id) = message_string(message, "use_id") else {
        return;
    };
    let kind = match message.event.as_str() {
        "start" => CortexEventKind::Start,
        "finish" => CortexEventKind::Finish,
        "error" => CortexEventKind::Error,
        _ => return,
    };
    let event = CortexOutcomeEvent {
        kind,
        use_id: use_id.to_owned(),
        provider: message_string(message, "provider").and_then(provider_name_from_wire),
        reason_code: message_string(message, "reason_code").map(str::to_owned),
    };
    let now = ProviderRuntimeNow {
        monotonic_seconds: state.started.elapsed().as_secs_f64(),
    };
    if kind == CortexEventKind::Start {
        let _ = state.wedge.observe(event, now);
        return;
    }
    if !state.wedge.is_tracked_local(use_id) || !local_endpoint_is_bundled(&state.journal) {
        return;
    }

    let mut failure_use_ids = state.wedge.failure_use_ids();
    if kind == CortexEventKind::Error && !failure_use_ids.iter().any(|id| id == use_id) {
        failure_use_ids.push(use_id.to_owned());
    }
    let Some(provider) = state.wedge.observe(event, now) else {
        return;
    };

    let Some(port) = read_local_port(&state.journal) else {
        eprintln!("supervisor: local wedge recycle deferred; local service port unavailable");
        return;
    };
    if !local_probe_is_ready(state) {
        eprintln!("supervisor: local wedge recycle deferred; local health is not ready");
        return;
    }
    if let Err(error) = request_local_provider_recycle(state, failure_use_ids, port) {
        eprintln!("supervisor: local wedge recycle request failed: {error:?}");
        return;
    }
    let mut sink = SupervisorProviderSink(state.server.clone());
    sink.emit(ProviderRuntimeEvent::RecycleRequested { provider });
}

fn provider_name_from_wire(value: &str) -> Option<ProviderName> {
    match value {
        "local" => Some(ProviderName::Local),
        "parakeet" => Some(ProviderName::Parakeet),
        _ => None,
    }
}

fn local_endpoint_is_bundled(journal: &Path) -> bool {
    let config = match read_journal_config(journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("supervisor: could not read local endpoint configuration: {error}");
            return false;
        }
    };
    matches!(
        resolve_local_endpoint(&config),
        LocalEndpointResolution::Bundled
    )
}

fn read_local_port(journal: &Path) -> Option<u16> {
    std::fs::read_to_string(journal.join("health/local.port"))
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn local_probe_is_ready(state: &SupervisorState) -> bool {
    // The supervisor fixture has no HTTP model server. Its synthetic launch is
    // already the established test-only local-runtime seam, so it stands in for
    // a healthy endpoint while production always uses the real ConnectOutcome.
    state.local.fixture_launch.is_some()
        || state.local.probe.probe_now(&state.local.state).status == ProbeStatus::Ready
}

fn request_local_provider_recycle(
    state: &mut SupervisorState,
    mut failure_use_ids: Vec<String>,
    port: u16,
) -> Result<(), RuntimeStoreError> {
    failure_use_ids.sort();
    let reason_code = ReasonCode::known("local-wedge-provider-unavailable");
    let desired_fingerprint = state.local.state.desired_fingerprint.clone();
    let token = match state.local.store.request_retry_token(
        desired_fingerprint.clone(),
        reason_code.clone(),
        Map::from_iter([
            ("module".into(), json!("solstone.think.supervisor")),
            ("source".into(), json!("provider-runtime-recycle")),
        ]),
    ) {
        Ok(token) => token,
        Err(error) => {
            state.local.state.latest_phase = store_error_phase(error.clone());
            return Err(error);
        }
    };

    let local = &mut state.local;
    local.state.generation += 1;
    local.state.retry = ProviderRetryState {
        desired_fingerprint,
        ..ProviderRetryState::default()
    };
    local.state.latest_phase = RuntimePhase::RetryRequested;
    local.state.latest_reason_code = Some(reason_code);
    local.state.latest_detail = Some(json!({
        "use_ids": failure_use_ids,
        "port": port,
        "health_state": "ready",
        "token_revision": token.revision,
    }));
    local.state.next_truth_at = 0.0;
    local.state.next_probe_at = 0.0;
    cancel_start(&mut local.state);
    if let Err(error) = local.store.publish_state(&local.state) {
        local.state.latest_phase = store_error_phase(error.clone());
        return Err(error);
    }
    Ok(())
}

fn message_string<'a>(message: &'a CallosumEnvelope, key: &str) -> Option<&'a str> {
    message
        .extra
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn message_truthy(message: &CallosumEnvelope, key: &str) -> bool {
    message.extra.get(key).is_some_and(|value| match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    })
}

fn sync_tick(state: &mut SupervisorState) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |value| value.as_secs_f64());
    let filename = state.heartbeat_filename.clone();
    let heartbeat = solstone_core_system::lifecycle::Heartbeat {
        schema: 1,
        machine_id: machine_id(),
        hostname: filename.trim_end_matches(".check").to_owned(),
        pid: std::process::id(),
        wall_time: now.to_string(),
        solstone_version: env!("CARGO_PKG_VERSION").to_owned(),
        interval_seconds: DEFAULT_INTERVAL_SECONDS as u32,
        journal_path: state.journal.display().to_string(),
    };
    if write_sync_heartbeat(
        &state.journal,
        &filename,
        &serde_json::to_vec(&heartbeat).expect("heartbeat serializes"),
    )
    .is_err()
    {
        return false;
    }
    let Ok(result) = check_sync(
        &state.journal,
        &filename,
        &heartbeat.machine_id,
        state.last_sync_snapshot.as_ref(),
        now,
    ) else {
        return false;
    };
    let conflict_now = result.is_tick_conflict(state.last_sync_snapshot.as_ref());
    state.stale_heartbeats = result
        .foreign_writers
        .iter()
        .filter(|writer| !writer.is_live)
        .cloned()
        .collect();
    state.last_sync_snapshot = Some(result.snapshot.clone());
    if !conflict_now {
        return false;
    }
    if let Some(conflict) = sync_conflict_event(&result) {
        emit(
            &state.server,
            "supervisor",
            "sync_conflict",
            Map::from_iter([
                ("hostname".into(), json!(conflict.hostname)),
                ("journal_path".into(), json!(conflict.journal_path)),
                ("pid".into(), json!(conflict.pid)),
                (
                    "machine_id_prefix".into(),
                    json!(conflict.machine_id_prefix),
                ),
                ("wall_time".into(), json!(conflict.wall_time)),
            ]),
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::NaiveDate;
    use solstone_core_system::cap::CapResolver;
    use solstone_core_system::partition::Partition;
    use solstone_core_system::queue::{
        ProcessState, ProcessStateProbe, TaskQueue, TaskQueueOptions,
    };

    use super::*;

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    struct Bed {
        root: PathBuf,
    }

    impl Bed {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "solstone-supervisor-tick-{name}-{}",
                NEXT_PATH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("temporary journal");
            fs::create_dir_all(root.join("config")).expect("config directory");
            Self { root }
        }

        fn enable_thinking(&self) {
            fs::write(
                self.root.join("config/journal.json"),
                br#"{"providers":{"active":{"provider":"local"}}}"#,
            )
            .expect("thinking config");
        }

        fn updated_day(&self, day: &str) {
            let path = self.root.join("chronicle").join(day).join("health");
            fs::create_dir_all(&path).expect("health directory");
            fs::write(path.join("stream.updated"), b"stream").expect("stream marker");
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct FixedCap;

    impl CapResolver for FixedCap {
        fn cap_for(&self, _partition: &Partition) -> Duration {
            Duration::from_secs(60)
        }
    }

    struct UnreachableProcessStateProbe;

    impl ProcessStateProbe for UnreachableProcessStateProbe {
        fn state(&self, _pid: u32) -> ProcessState {
            panic!("routine queue unit tests must not reach the process-state probe");
        }
    }

    fn queue(root: &std::path::Path) -> TaskQueue {
        TaskQueue::new(TaskQueueOptions {
            journal_root: root.to_path_buf(),
            cap_resolver: Arc::new(FixedCap),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: false,
            before_deadline_commit: None,
        })
    }

    #[test]
    fn app_observation_requires_a_captured_start_and_preserves_poll_outcomes() {
        let started_at = Instant::now();
        let now = started_at + Duration::from_secs(4);
        let live = observe_app_process(
            AppProcessSample {
                service: AppService::Convey,
                process_count: 1,
                tuple: Some(ProcessObservationTuple {
                    reference: "supervisor-app-convey".into(),
                    pid: 11,
                    started_at,
                    poll: Ok(None),
                }),
            },
            now,
        );
        assert_eq!(
            live,
            SystemProcessObservation::Live {
                reference: "supervisor-app-convey".into(),
                pid: 11,
                uptime_seconds: 4,
            }
        );
        assert_eq!(
            observe_app_process(
                AppProcessSample {
                    service: AppService::Convey,
                    process_count: 1,
                    tuple: None,
                },
                now,
            ),
            SystemProcessObservation::Indeterminate
        );
        assert_eq!(
            observe_app_process(
                AppProcessSample {
                    service: AppService::Convey,
                    process_count: 1,
                    tuple: Some(ProcessObservationTuple {
                        reference: "supervisor-app-convey".into(),
                        pid: 11,
                        started_at,
                        poll: Ok(Some(0)),
                    }),
                },
                now,
            ),
            SystemProcessObservation::ConfirmedAbsent
        );
    }

    fn live_observation(reference: &str, pid: u32) -> SystemProcessObservation {
        SystemProcessObservation::Live {
            reference: reference.to_owned(),
            pid,
            uptime_seconds: 4,
        }
    }

    fn provider_state(provider: ProviderName, phase: RuntimePhase) -> ProviderRuntimeState {
        let mut state = ProviderRuntimeState::new(provider);
        state.latest_phase = phase;
        state
    }

    fn empty_queue_snapshot() -> TaskQueueStatusSnapshot {
        TaskQueueStatusSnapshot {
            tasks: Vec::new(),
            recent_tasks: Vec::new(),
            queues: BTreeMap::new(),
        }
    }

    #[test]
    fn status_emission_plan_composes_a_status_only_when_all_observations_are_determinate() {
        let local = provider_state(ProviderName::Local, RuntimePhase::Ready);
        let parakeet = provider_state(ProviderName::Parakeet, RuntimePhase::Stopped);

        let plan = plan_status_emission(StatusEmissionInputs {
            app_observations: vec![(
                AppService::Convey,
                live_observation("supervisor-app-convey", 11),
            )],
            app_crashed: Vec::new(),
            local_observation: live_observation("local:12", 12),
            parakeet_observation: SystemProcessObservation::ConfirmedAbsent,
            local_state: &local,
            parakeet_state: &parakeet,
            supervisor_pid: 10,
            supervisor_uptime_seconds: 8,
            queue: empty_queue_snapshot(),
            stale_heartbeats: Vec::new(),
            schedules: Vec::new(),
            callosum_clients: 2,
        });

        let StatusEmissionPlan::Status(input) = plan else {
            panic!("determinate observations must produce a status plan");
        };
        assert_eq!(input.services.len(), 4);
        assert!(matches!(
            &input.services[0],
            ServiceCandidate::SupervisorSelf {
                reference,
                pid: 10,
                uptime_seconds: 8,
            } if reference == "supervisor"
        ));
        assert!(matches!(
            &input.services[1],
            ServiceCandidate::App { name, .. } if name == "convey"
        ));
        assert!(matches!(
            &input.services[2],
            ServiceCandidate::Provider {
                provider: ProviderName::Local,
                phase: RuntimePhase::Ready,
                ..
            }
        ));
    }

    #[test]
    fn status_emission_plan_suppresses_status_for_indeterminate_observations() {
        let local = provider_state(ProviderName::Local, RuntimePhase::Ready);
        let parakeet = provider_state(ProviderName::Parakeet, RuntimePhase::Ready);
        let app_plan = plan_status_emission(StatusEmissionInputs {
            app_observations: vec![(AppService::Convey, SystemProcessObservation::Indeterminate)],
            app_crashed: Vec::new(),
            local_observation: live_observation("local:12", 12),
            parakeet_observation: live_observation("parakeet:13", 13),
            local_state: &local,
            parakeet_state: &parakeet,
            supervisor_pid: 10,
            supervisor_uptime_seconds: 8,
            queue: empty_queue_snapshot(),
            stale_heartbeats: Vec::new(),
            schedules: Vec::new(),
            callosum_clients: 2,
        });
        assert!(matches!(app_plan, StatusEmissionPlan::Errors(services) if services == ["convey"]));

        let provider_plan = plan_status_emission(StatusEmissionInputs {
            app_observations: Vec::new(),
            app_crashed: Vec::new(),
            local_observation: SystemProcessObservation::Indeterminate,
            parakeet_observation: live_observation("parakeet:13", 13),
            local_state: &local,
            parakeet_state: &parakeet,
            supervisor_pid: 10,
            supervisor_uptime_seconds: 8,
            queue: empty_queue_snapshot(),
            stale_heartbeats: Vec::new(),
            schedules: Vec::new(),
            callosum_clients: 2,
        });
        assert!(
            matches!(provider_plan, StatusEmissionPlan::Errors(services) if services == ["local"])
        );
    }

    #[test]
    fn status_emission_plan_uses_retry_attempts_for_crashed_provider_rows() {
        let mut local = provider_state(ProviderName::Local, RuntimePhase::Ready);
        local.retry.attempt_count = 3;
        local.cleanup_attempt_count = 99;
        let mut parakeet = provider_state(ProviderName::Parakeet, RuntimePhase::CleanupFailed);
        parakeet.retry.attempt_count = 3;
        parakeet.cleanup_attempt_count = 99;

        let plan = plan_status_emission(StatusEmissionInputs {
            app_observations: Vec::new(),
            app_crashed: Vec::new(),
            local_observation: live_observation("local:12", 12),
            parakeet_observation: live_observation("parakeet:13", 13),
            local_state: &local,
            parakeet_state: &parakeet,
            supervisor_pid: 10,
            supervisor_uptime_seconds: 8,
            queue: empty_queue_snapshot(),
            stale_heartbeats: Vec::new(),
            schedules: Vec::new(),
            callosum_clients: 2,
        });
        let StatusEmissionPlan::Status(input) = plan else {
            panic!("determinate observations must produce a status plan");
        };

        let projected = project_supervisor_status(input);
        let services = projected["services"].as_array().expect("services array");
        assert!(services.iter().any(|service| {
            service["name"].as_str() == Some("local") && service["phase"].as_str() == Some("ready")
        }));
        assert!(services.iter().any(|service| {
            service["name"].as_str() == Some("parakeet")
                && service["phase"].as_str() == Some("cleanup-failed")
        }));
        let crashed = projected["crashed"].as_array().expect("crashed array");
        assert_eq!(crashed.len(), 1);
        assert_eq!(crashed[0]["name"].as_str(), Some("parakeet"));
        assert_eq!(crashed[0]["restart_attempts"].as_u64(), Some(3));
    }

    fn pending(queue: &TaskQueue) -> usize {
        queue
            .collect_queue_counts()
            .get("pending")
            .copied()
            .unwrap_or(0)
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, day).expect("fixture date")
    }

    #[test]
    fn check_segment_flush_forces_expected_command_and_marks_state() {
        let bed = Bed::new("forced-flush");
        bed.enable_thinking();
        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut flush = FlushState {
            last_segment_ts: Some(origin),
            day: Some("20260101".to_owned()),
            segment: Some("120000_1".to_owned()),
            stream: Some("camera".to_owned()),
            flushed: false,
        };

        check_segment_flush(&bed.root, &queue, false, &mut flush, true, origin);

        assert!(flush.flushed);
        assert_eq!(pending(&queue), 1);
        assert_eq!(
            flush_think_argv("20260101", "120000_1", Some("camera")),
            [
                "journal",
                "think",
                "-v",
                "--day",
                "20260101",
                "--segment",
                "120000_1",
                "--flush",
                "--stream",
                "camera",
            ]
            .map(str::to_owned)
        );

        let mut flush = FlushState {
            last_segment_ts: Some(origin),
            day: Some("20260101".to_owned()),
            segment: Some("120000_1".to_owned()),
            stream: Some("camera".to_owned()),
            flushed: false,
        };
        check_segment_flush(
            &bed.root,
            &queue,
            false,
            &mut flush,
            false,
            origin + FLUSH_TIMEOUT - Duration::from_secs(1),
        );
        assert!(!flush.flushed);
        assert_eq!(pending(&queue), 1);

        check_segment_flush(
            &bed.root,
            &queue,
            false,
            &mut flush,
            false,
            origin + FLUSH_TIMEOUT,
        );
        assert!(flush.flushed);
        assert_eq!(pending(&queue), 2);

        let mut flush = FlushState {
            last_segment_ts: Some(origin),
            day: Some("20260101".to_owned()),
            segment: Some("120000_1".to_owned()),
            stream: Some("camera".to_owned()),
            flushed: false,
        };
        check_segment_flush(
            &bed.root,
            &queue,
            false,
            &mut flush,
            false,
            origin + FLUSH_TIMEOUT + Duration::from_secs(1),
        );
        assert!(flush.flushed);
        assert_eq!(pending(&queue), 3);
    }

    #[test]
    fn check_segment_flush_is_a_remote_mode_noop() {
        let bed = Bed::new("flush-remote");
        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut flush = FlushState {
            last_segment_ts: Some(origin),
            day: Some("20260101".to_owned()),
            segment: Some("120000_1".to_owned()),
            stream: None,
            flushed: false,
        };

        check_segment_flush(
            &bed.root,
            &queue,
            true,
            &mut flush,
            true,
            origin + FLUSH_TIMEOUT + Duration::from_secs(1),
        );

        assert!(!flush.flushed);
        assert_eq!(pending(&queue), 0);
    }

    fn assert_daily_rollover(name: &str) {
        let bed = Bed::new(name);
        bed.enable_thinking();
        for day in [
            "20260101", "20260102", "20260103", "20260104", "20260105", "20260106",
        ] {
            bed.updated_day(day);
        }
        let queue = queue(&bed.root);
        let mut daily = DailyState {
            last_day: Some(date(6)),
        };
        let mut flush = FlushState {
            last_segment_ts: Some(Instant::now()),
            day: Some("20260106".to_owned()),
            segment: Some("120000_1".to_owned()),
            stream: None,
            flushed: false,
        };

        handle_daily_tasks(
            &bed.root,
            &queue,
            false,
            &mut daily,
            &mut flush,
            date(7),
            UNIX_EPOCH,
        )
        .expect("daily rollover");

        assert_eq!(daily.last_day, Some(date(7)));
        assert!(flush.flushed);
        assert_eq!(pending(&queue), 5);
        assert_eq!(
            daily_think_argv("20260106"),
            ["journal", "think", "-v", "--day", "20260106"].map(str::to_owned)
        );
    }

    #[test]
    fn handle_daily_tasks_rollover_forces_flush_and_caps_catchup() {
        assert_daily_rollover("rollover");
    }

    #[test]
    fn handle_daily_tasks_rollover_does_not_require_schedule_engine() {
        // `--no-schedule` is represented by no ScheduleEngine being created.
        // This direct daily fixture therefore exercises the rollover work with
        // no scheduler dependency at all.
        assert_daily_rollover("rollover-no-schedule");
    }

    #[test]
    fn handle_daily_tasks_with_no_previous_day_warns_and_skips() {
        let bed = Bed::new("missing-day");
        bed.enable_thinking();
        bed.updated_day("20260101");
        let queue = queue(&bed.root);
        let mut daily = DailyState { last_day: None };
        let mut flush = FlushState::default();

        handle_daily_tasks(
            &bed.root,
            &queue,
            false,
            &mut daily,
            &mut flush,
            date(2),
            UNIX_EPOCH,
        )
        .expect("missing previous day");

        assert_eq!(daily.last_day, Some(date(2)));
        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn handle_daily_tasks_is_a_remote_mode_noop() {
        let bed = Bed::new("daily-remote");
        let queue = queue(&bed.root);
        let mut daily = DailyState {
            last_day: Some(date(1)),
        };
        let mut flush = FlushState::default();

        handle_daily_tasks(
            &bed.root,
            &queue,
            true,
            &mut daily,
            &mut flush,
            date(2),
            UNIX_EPOCH,
        )
        .expect("remote daily no-op");

        assert_eq!(daily.last_day, Some(date(1)));
        assert_eq!(pending(&queue), 0);
    }

    fn request_with_cmd(cmd: Value) -> CallosumEnvelope {
        CallosumEnvelope {
            tract: "supervisor".into(),
            event: "request".into(),
            ts: None,
            extra: Map::from_iter([("cmd".into(), cmd)]),
        }
    }

    #[test]
    fn decode_supervisor_cmd_names_malformed_requests() {
        let missing = CallosumEnvelope {
            tract: "supervisor".into(),
            event: "request".into(),
            ts: None,
            extra: Map::new(),
        };
        assert!(matches!(
            decode_supervisor_cmd(&missing),
            Err(SupervisorRequestError::MissingCmd)
        ));
        assert!(matches!(
            decode_supervisor_cmd(&request_with_cmd(json!("journal"))),
            Err(SupervisorRequestError::MissingCmd)
        ));
        assert!(matches!(
            decode_supervisor_cmd(&request_with_cmd(json!([1, "brain"]))),
            Err(SupervisorRequestError::NonStringElement)
        ));
        assert!(matches!(
            decode_supervisor_cmd(&request_with_cmd(json!([]))),
            Err(SupervisorRequestError::EmptyCmd)
        ));
    }

    #[test]
    fn decode_supervisor_cmd_accepts_literal_and_resolved_journal_argv() {
        assert!(matches!(
            decode_supervisor_cmd(&request_with_cmd(json!(["journal", "brain", "refresh"]))),
            Ok(TaskArgv::Brain(_))
        ));
        assert!(matches!(
            decode_supervisor_cmd(&request_with_cmd(json!([
                "/opt/sol/solstone-core-journal",
                "brain",
                "refresh"
            ]))),
            Ok(TaskArgv::Unknown { .. })
        ));
    }
}
