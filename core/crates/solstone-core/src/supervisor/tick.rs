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
    DEFAULT_INTERVAL_SECONDS, ParentLossReason, ParentWatch, ParentWatchStatus,
    SupervisorLifecycle, SyncPeerObservation, SyncTickOutcome, sync_conflict_event,
    sync_peer_diagnostic,
};
use solstone_core_system::process::{ProcessInstanceSource, SystemProcessInstanceSource};
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
use solstone_core_system::request::{
    BusTaskRequest, DailyCatchupProvenance, ExecutionRequest, TaskArgv,
};
use solstone_core_system::schedule::{ScheduleNow, ScheduleStatus};
use solstone_core_system::status_wire::{
    CrashedServiceCandidate, ProcessObservation as WireProcessObservation, ServiceCandidate,
    StaleHeartbeatWireInput, SupervisorStatusWireInput, project_supervisor_status,
};
use solstone_core_system::{
    catchup::{
        CatchupError, days_with_expired_retry, eligible_catchup_days,
        reconcile_stale_catchup_attempts,
    },
    queue::{SubmitOutcome, TaskQueue, TaskQueueStatusSnapshot},
};

use super::bus::{SupervisorProviderSink, SupervisorScheduleSink, emit};
use super::config::{no_thinking_engine_chosen, processing_is_deferred};
use super::runtime::{
    AppExit, AppService, DailyState, FlushState, ManagedAppProcess, SupervisorState, apply_app_exit,
};

const MAX_INBOUND_PER_TICK: usize = 256;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3600);
pub(crate) const RETRY_EXPIRY_INTERVAL: Duration = Duration::from_secs(60);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupervisorSignal {
    SigTerm,
    SigInt,
}

#[derive(Debug)]
pub(crate) enum SupervisorStopReason {
    Signal(SupervisorSignal),
    Sync(SyncTickOutcome),
    ParentLost(ParentLossReason),
}

fn check_parent_watch(
    parent_watch: Option<&ParentWatch>,
    source: &dyn ProcessInstanceSource,
) -> Option<SupervisorStopReason> {
    let watch = parent_watch?;
    match watch.check(source) {
        ParentWatchStatus::Live => None,
        ParentWatchStatus::Lost(reason) => Some(SupervisorStopReason::ParentLost(reason)),
    }
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

    async fn wait(&mut self) -> SupervisorSignal {
        #[cfg(unix)]
        tokio::select! {
            _ = self.terminate.recv() => SupervisorSignal::SigTerm,
            _ = self.interrupt.recv() => SupervisorSignal::SigInt,
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            SupervisorSignal::SigInt
        }
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

pub(crate) async fn run(
    state: &mut SupervisorState,
    lifecycle: &mut SupervisorLifecycle,
    shutdown: &mut ShutdownSignals,
    parent_watch: Option<ParentWatch>,
) -> SupervisorStopReason {
    let mut last_status = Instant::now() - state.timing.status_interval;
    let mut last_sync = Instant::now() - Duration::from_secs_f64(DEFAULT_INTERVAL_SECONDS);
    loop {
        if let Some(reason) =
            check_parent_watch(parent_watch.as_ref(), &SystemProcessInstanceSource)
        {
            return reason;
        }
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
        if !state.no_daily {
            let daily_drain = match handle_daily_tasks(
                &state.journal,
                &state.queue,
                state.is_remote_mode,
                &mut state.daily,
                &mut state.flush,
                wall.date_naive(),
                wall_now,
            ) {
                Ok(did_drain) => did_drain,
                Err(error) => {
                    eprintln!("supervisor: daily catchup drain failed: {error}");
                    false
                }
            };
            if daily_drain {
                // The rollover drain has just considered the same state; do
                // not immediately replay it through the retry-expiry path.
                state.last_retry_expiry_drain = tick;
            } else if let Err(error) = handle_retry_expiry_drain(
                state.is_remote_mode,
                processing_is_deferred(&state.journal),
                &state.journal,
                &state.queue,
                &mut state.last_retry_expiry_drain,
                wall.date_naive(),
                tick,
                wall_now,
            ) {
                eprintln!("supervisor: retry-expiry catchup drain failed: {error}");
            }
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
            let outcome = sync_tick(state, lifecycle);
            if !matches!(outcome, SyncTickOutcome::Healthy) {
                return SupervisorStopReason::Sync(outcome);
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
            signal = shutdown.wait() => return SupervisorStopReason::Signal(signal),
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
) -> Result<bool, CatchupError> {
    if is_remote || daily.last_day == Some(today) {
        return Ok(false);
    }
    let Some(previous_day) = daily.last_day else {
        eprintln!("supervisor: daily state not initialized; skipping daily processing");
        daily.last_day = Some(today);
        return Ok(false);
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
    )?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)] // The tick's clock/watermark seams remain explicitly injectable.
fn handle_retry_expiry_drain(
    is_remote: bool,
    is_deferred: bool,
    journal: &Path,
    queue: &TaskQueue,
    last_drain: &mut Instant,
    today: chrono::NaiveDate,
    tick: Instant,
    now: SystemTime,
) -> Result<(), CatchupError> {
    if is_remote || is_deferred {
        return Ok(());
    }
    if tick.saturating_duration_since(*last_drain) < RETRY_EXPIRY_INTERVAL {
        return Ok(());
    }
    *last_drain = tick;
    let exclude = BTreeSet::from([today.format("%Y%m%d").to_string()]);
    let expired_days = days_with_expired_retry(journal, &exclude, now)?;
    if !expired_days.is_empty() {
        // Expiry wakes the ordinary automatic selector. It is not an owner
        // force: dirty-day filtering, both catchup gates, and the four-day cap
        // remain authoritative.
        run_catchup_drain(journal, queue, &exclude, &[], now)?;
    }
    Ok(())
}

/// Reconcile durable crash leftovers, then make one normal automatic pass
/// before the retry timer begins.  This is intentionally independent of the
/// queue's transient worker history.
pub(crate) fn initialize_catchup(
    journal: &Path,
    queue: &TaskQueue,
    is_remote: bool,
    no_daily: bool,
    today: chrono::NaiveDate,
    now: SystemTime,
) -> Result<(), CatchupError> {
    initialize_catchup_with_reconcile(
        journal,
        queue,
        is_remote,
        no_daily,
        today,
        now,
        reconcile_stale_catchup_attempts,
    )
}

pub(crate) fn initialize_catchup_with_reconcile<Reconcile>(
    journal: &Path,
    queue: &TaskQueue,
    is_remote: bool,
    no_daily: bool,
    today: chrono::NaiveDate,
    now: SystemTime,
    reconcile: Reconcile,
) -> Result<(), CatchupError>
where
    Reconcile: FnOnce(&Path, SystemTime) -> Result<(), CatchupError>,
{
    reconcile(journal, now)?;
    if is_remote || no_daily || processing_is_deferred(journal) {
        return Ok(());
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
        let reference = format!("supervisor-catchup-{day}");
        let provenance = DailyCatchupProvenance { day: day.clone() };
        let _ = submit_catchup_think(queue, daily_think_argv(&day), &day, reference, provenance);
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
    submit_task(queue, argv, reference, Some(day), None)
}

fn submit_catchup_think(
    queue: &TaskQueue,
    argv: Vec<String>,
    day: &str,
    reference: String,
    provenance: DailyCatchupProvenance,
) -> solstone_core_system::queue::SubmitOutcome {
    submit_task(queue, argv, reference, Some(day), Some(provenance))
}

fn submit_task(
    queue: &TaskQueue,
    argv: Vec<String>,
    reference: String,
    day: Option<&str>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
) -> solstone_core_system::queue::SubmitOutcome {
    let cmd = TaskArgv::from_wire(argv).expect("supervisor constructs a non-empty think argv");
    queue.submit(ExecutionRequest::Bus(BusTaskRequest {
        cmd,
        reference,
        day: day.map(str::to_owned),
        scheduler_name: None,
        queue_if_active_cmd_differs: false,
        daily_catchup_provenance,
    }))
}

fn reconcile_app_processes(state: &mut SupervisorState) -> Vec<AppProcessSample> {
    let journal = state.journal.clone();
    let server = state.server.clone();
    let sense_child_environment = state.sense_child_environment.clone();
    let mut samples = Vec::new();
    for app in &mut state.app_processes {
        if !app.enabled {
            continue;
        }
        if app.process.is_none()
            && app
                .restart_at
                .is_some_and(|restart_at| Instant::now() >= restart_at)
            && let Err(error) = super::runtime::spawn_app_process(
                app,
                &journal,
                server.clone(),
                &sense_child_environment,
            )
        {
            eprintln!(
                "supervisor: failed to restart {}: {error}",
                app.service.as_str()
            );
            apply_app_exit(app, &journal, AppExit::SpawnFailure);
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
        samples.push(AppProcessSample {
            service: app.service,
            process_count: 0,
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

fn stale_heartbeat_wire_input(writer: &SyncPeerObservation) -> StaleHeartbeatWireInput {
    let diagnostic = sync_peer_diagnostic(writer);
    let malformed = diagnostic.identity.is_unidentified();
    StaleHeartbeatWireInput {
        source_filename: writer.source_filename.as_encoded_bytes().to_vec(),
        hostname: diagnostic.hostname,
        identity: diagnostic.identity,
        journal_path: diagnostic.journal_path,
        pid: diagnostic.pid,
        wall_time: diagnostic.wall_time,
        malformed,
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
        daily_catchup_provenance: None,
    };
    if state.queue.submit(ExecutionRequest::Bus(request)) == SubmitOutcome::Rejected {
        eprintln!("supervisor: request rejected");
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
        None,
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

fn sync_tick(state: &mut SupervisorState, lifecycle: &mut SupervisorLifecycle) -> SyncTickOutcome {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |value| value.as_secs_f64());
    let outcome = lifecycle.tick_sync(state.last_sync_snapshot.as_ref(), now);
    match &outcome {
        SyncTickOutcome::Healthy => {
            update_completed_sync_state(state, lifecycle);
        }
        SyncTickOutcome::Conflict(result) => {
            update_completed_sync_state(state, lifecycle);
            eprintln!("supervisor: sync conflict");
            if let Some(conflict) = sync_conflict_event(result) {
                let mut fields = Map::from_iter([
                    ("hostname".into(), json!(conflict.hostname)),
                    ("journal_path".into(), json!(conflict.journal_path)),
                    ("pid".into(), json!(conflict.pid)),
                    ("wall_time".into(), json!(conflict.wall_time)),
                    (
                        "heartbeat_schema".into(),
                        json!(conflict.identity.schema_name()),
                    ),
                ]);
                if let Some(prefix) = conflict.identity.legacy_machine_id_prefix() {
                    fields.insert("legacy_machine_id_prefix".into(), json!(prefix));
                }
                if let Some(prefix) = conflict.identity.writer_id_prefix() {
                    fields.insert("writer_id_prefix".into(), json!(prefix));
                }
                if let Some(run_id) = conflict.identity.run_id() {
                    fields.insert("run_id".into(), json!(run_id));
                }
                emit(&state.server, "supervisor", "sync_conflict", fields);
            }
        }
        SyncTickOutcome::RenewalFailure(error) => {
            eprintln!("supervisor: sync renewal failure");
            eprintln!("supervisor: sync renewal failure detail: {error:?}");
        }
        SyncTickOutcome::CompleteScanFailure(error) => {
            eprintln!("supervisor: sync complete scan failure");
            eprintln!("supervisor: sync complete scan failure detail: {error:?}");
        }
        SyncTickOutcome::RetainedObservationFailure(error) => {
            eprintln!("supervisor: sync retained observation failure");
            eprintln!("supervisor: sync retained observation failure detail: {error:?}");
        }
        SyncTickOutcome::StaleHeartbeatCollectionFailure(error) => {
            eprintln!("supervisor: stale heartbeat collection failure");
            eprintln!("supervisor: stale heartbeat collection failure detail: {error:?}");
        }
    }
    outcome
}

fn update_completed_sync_state(state: &mut SupervisorState, lifecycle: &SupervisorLifecycle) {
    let result = lifecycle
        .last_completed_sync_result()
        .expect("healthy and conflict outcomes retain a completed sync result");
    state.stale_heartbeats = result
        .peer_observations
        .iter()
        .filter(|peer| !peer.is_live)
        .cloned()
        .collect();
    state.last_sync_snapshot = Some(result.snapshot.clone());
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::NaiveDate;
    use solstone_core_system::cap::CapResolver;
    use solstone_core_system::lifecycle::{
        HeartbeatClassification, HeartbeatV2, RunId, SyncPeerIdentity, WriterId,
    };
    use solstone_core_system::partition::Partition;
    use solstone_core_system::process::{
        ExecutionState, InspectResult, InstanceCensus, ProcessBirth, ProcessInstance,
        ProcessInstanceSource,
    };
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
            child_environment: BTreeMap::new(),
        })
    }

    struct ParentAdmissionSource {
        self_result: InspectResult,
        parent_result: InspectResult,
    }

    impl ProcessInstanceSource for ParentAdmissionSource {
        fn inspect(&self, pid: u32) -> InspectResult {
            if pid == std::process::id() {
                self.self_result
            } else {
                self.parent_result
            }
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    struct ParentCheckSource {
        result: InspectResult,
    }

    impl ProcessInstanceSource for ParentCheckSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            self.result
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    fn parent_instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn admitted_parent_watch() -> (ParentWatch, ProcessInstance) {
        let parent = parent_instance(42, 10);
        let source = ParentAdmissionSource {
            self_result: InspectResult::Present {
                instance: parent_instance(std::process::id(), 1),
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(parent.pid),
                pgid: None,
            },
            parent_result: InspectResult::Present {
                instance: parent,
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: None,
            },
        };
        (
            ParentWatch::admit(
                solstone_core_system::lifecycle::DeclaredParent::from_instance(parent),
                &source,
            )
            .expect("parent admission"),
            parent,
        )
    }

    #[test]
    fn parent_watch_check_maps_live_and_lost_observations() {
        let (watch, parent) = admitted_parent_watch();
        let live = ParentCheckSource {
            result: InspectResult::Present {
                instance: parent,
                uid: 501,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: None,
            },
        };
        assert!(check_parent_watch(Some(&watch), &live).is_none());

        let unverifiable = ParentCheckSource {
            result: InspectResult::Unverifiable,
        };
        assert!(matches!(
            check_parent_watch(Some(&watch), &unverifiable),
            Some(SupervisorStopReason::ParentLost(
                ParentLossReason::Unverifiable
            ))
        ));

        let exited = ParentCheckSource {
            result: InspectResult::Absent,
        };
        assert!(matches!(
            check_parent_watch(Some(&watch), &exited),
            Some(SupervisorStopReason::ParentLost(
                ParentLossReason::ExitedOrReused
            ))
        ));
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
    fn stale_v2_peer_projects_schema_discriminated_identity() {
        let heartbeat = HeartbeatV2::new(
            WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID"),
            RunId::parse("fedcba9876543210fedcba9876543210").expect("run ID"),
            "foreign-host".to_owned(),
            42,
            "1234.5".to_owned(),
            "test".to_owned(),
            15,
            "/foreign-journal".to_owned(),
        );
        let input = stale_heartbeat_wire_input(&SyncPeerObservation {
            source_filename: OsString::from("foreign.check"),
            classification: HeartbeatClassification::SchemaV2(heartbeat),
            heartbeat: None,
            is_live: false,
        });

        assert_eq!(input.hostname, "foreign-host");
        assert_eq!(input.journal_path, "/foreign-journal");
        assert_eq!(input.pid, Some(42));
        assert_eq!(input.wall_time.as_deref(), Some("1234.5"));
        assert!(!input.malformed);
        assert_eq!(
            input.identity,
            SyncPeerIdentity::V2 {
                writer_id_prefix: "01234567".to_owned(),
                run_id: "fedcba9876543210fedcba9876543210".to_owned(),
            }
        );
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
    fn retry_expiry_drain_throttles_and_excludes_today() {
        let bed = Bed::new("retry-expiry");
        bed.enable_thinking();
        for day in ["20260101", "20260103"] {
            bed.updated_day(day);
        }
        fs::create_dir_all(bed.root.join("health")).expect("health directory");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    "20260101:daily-catchup": {
                        "day": "20260101",
                        "command_kind": "daily-catchup",
                        "active": null,
                        "next_retry_at": 10.0,
                    },
                    "20260103:segment-repair": {
                        "day": "20260103",
                        "command_kind": "segment-repair",
                        "active": null,
                        "next_retry_at": 10.0,
                    },
                },
            }))
            .expect("retry state"),
        )
        .expect("write retry state");

        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut last_drain = origin;
        let now = UNIX_EPOCH + Duration::from_secs(10);

        handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(3),
            origin + RETRY_EXPIRY_INTERVAL - Duration::from_secs(1),
            now,
        )
        .expect("early retry tick");
        assert_eq!(pending(&queue), 0);

        handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(3),
            origin + RETRY_EXPIRY_INTERVAL,
            now,
        )
        .expect("expired retry tick");
        assert_eq!(pending(&queue), 1, "only the non-today retry is drained");

        handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(3),
            origin + RETRY_EXPIRY_INTERVAL + Duration::from_secs(1),
            now,
        )
        .expect("throttled retry tick");
        assert_eq!(pending(&queue), 1, "the same window must not replay");
    }

    #[test]
    fn retry_expiry_wakes_the_newest_four_automatic_days_without_bypassing_the_cap() {
        let bed = Bed::new("retry-expiry-cap");
        bed.enable_thinking();
        let days = [
            "20260101", "20260102", "20260103", "20260104", "20260105", "20260106",
        ];
        let mut entries = Map::new();
        for day in days {
            bed.updated_day(day);
            entries.insert(
                format!("{day}:daily-catchup"),
                json!({
                    "day": day,
                    "command_kind": "daily-catchup",
                    "active": null,
                    "next_retry_at": 10.0,
                }),
            );
        }
        fs::create_dir_all(bed.root.join("health")).expect("health directory");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            serde_json::to_vec(&json!({"version": 1, "entries": entries})).expect("retry state"),
        )
        .expect("write retry state");
        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut last_drain = origin;

        handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(7),
            origin + RETRY_EXPIRY_INTERVAL,
            UNIX_EPOCH + Duration::from_secs(10),
        )
        .expect("expired retry tick");

        assert_eq!(pending(&queue), 4);
    }

    #[test]
    fn retry_expiry_does_not_force_a_day_whose_marker_pair_is_already_clean() {
        let bed = Bed::new("retry-expiry-clean");
        bed.enable_thinking();
        let health = bed.root.join("chronicle/20260101/health");
        fs::create_dir_all(&health).expect("health directory");
        fs::write(
            health.join("stream.updated"),
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        )
        .expect("stream marker");
        fs::write(
            health.join("daily.updated"),
            br#"{"version":1,"generation":1,"fingerprint":"complete"}"#,
        )
        .expect("daily marker");
        fs::create_dir_all(bed.root.join("health")).expect("catchup health");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    "20260101:daily-catchup": {
                        "day": "20260101",
                        "command_kind": "daily-catchup",
                        "active": null,
                        "next_retry_at": 10.0,
                    }
                }
            }))
            .expect("retry state"),
        )
        .expect("write retry state");
        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut last_drain = origin;

        handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(2),
            origin + RETRY_EXPIRY_INTERVAL,
            UNIX_EPOCH + Duration::from_secs(10),
        )
        .expect("expired retry tick");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn retry_expiry_drain_is_a_remote_or_deferred_mode_noop() {
        let bed = Bed::new("retry-expiry-remote");
        bed.enable_thinking();
        fs::create_dir_all(bed.root.join("chronicle/20260101")).expect("chronicle day");
        fs::create_dir_all(bed.root.join("health")).expect("health directory");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    "20260101:daily-catchup": {
                        "day": "20260101",
                        "command_kind": "daily-catchup",
                        "active": null,
                        "next_retry_at": 10.0,
                    },
                },
            }))
            .expect("retry state"),
        )
        .expect("write retry state");

        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut last_drain = origin;
        handle_retry_expiry_drain(
            true,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(2),
            origin + RETRY_EXPIRY_INTERVAL,
            UNIX_EPOCH + Duration::from_secs(10),
        )
        .expect("remote retry tick");

        assert_eq!(pending(&queue), 0);
        assert_eq!(last_drain, origin);

        handle_retry_expiry_drain(
            false,
            true,
            &bed.root,
            &queue,
            &mut last_drain,
            date(2),
            origin + RETRY_EXPIRY_INTERVAL,
            UNIX_EPOCH + Duration::from_secs(10),
        )
        .expect("deferred retry tick");

        assert_eq!(pending(&queue), 0);
        assert_eq!(last_drain, origin);
    }

    #[test]
    fn startup_reconciles_before_draining_dirty_days_and_excludes_today() {
        let bed = Bed::new("startup-catchup");
        bed.enable_thinking();
        for day in ["20260101", "20260102", "20260103"] {
            let health = bed.root.join("chronicle").join(day).join("health");
            fs::create_dir_all(&health).expect("health directory");
            fs::write(
                health.join("stream.updated"),
                br#"{"version":1,"generation":1,"fingerprint":null}"#,
            )
            .expect("stream marker");
        }
        fs::create_dir_all(bed.root.join("health")).expect("catchup health");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    "20260101:daily-catchup": {
                        "day": "20260101",
                        "command_kind": "daily-catchup",
                        "active": {"ref": "lost", "started_at": 1.0},
                        "admitted_generation": 1,
                        "fingerprint": solstone_core_system::catchup::read_raw_input_fingerprint(
                            &bed.root,
                            "20260101",
                        )
                        .unwrap(),
                        "next_retry_at": 0.0,
                    }
                }
            }))
            .expect("catchup state"),
        )
        .expect("write catchup state");
        let queue = queue(&bed.root);

        initialize_catchup(
            &bed.root,
            &queue,
            false,
            false,
            date(3),
            UNIX_EPOCH + Duration::from_secs(20),
        )
        .expect("startup catchup");

        assert_eq!(pending(&queue), 1, "only fresh past-day dirtiness drains");
        let state: Value = serde_json::from_slice(
            &fs::read(bed.root.join("health/catchup-state.json")).expect("catchup state"),
        )
        .expect("catchup JSON");
        let stale = &state["entries"]["20260101:daily-catchup"];
        assert_eq!(stale["last_outcome"], "interrupted");
        assert_eq!(stale["next_retry_at"], 620.0);
    }

    #[test]
    fn capability_refusal_stops_startup_catchup_before_queue_drain_or_ledger_mutation() {
        let bed = Bed::new("startup-catchup-capability");
        bed.enable_thinking();
        let day = "20260101";
        let health = bed.root.join("chronicle").join(day).join("health");
        fs::create_dir_all(&health).expect("health directory");
        fs::write(
            health.join("stream.updated"),
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        )
        .expect("stream marker");
        let state_path = bed.root.join("health/catchup-state.json");
        fs::create_dir_all(state_path.parent().expect("health directory")).expect("health");
        fs::write(
            &state_path,
            br#"{"version":1,"entries":{"20260101:daily-catchup":{"day":"20260101","command_kind":"daily-catchup","active":{"ref":"lost","started_at":1.0}}}}"#,
        )
        .expect("catchup state");
        let before = fs::read(&state_path).expect("catchup state");
        let queue = queue(&bed.root);

        let result = initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(20),
            |_, _| Err(CatchupError::CapabilityUnavailable),
        );

        assert!(matches!(result, Err(CatchupError::CapabilityUnavailable)));
        assert_eq!(pending(&queue), 0, "refusal must precede queue drain");
        assert_eq!(fs::read(&state_path).expect("catchup state"), before);
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
