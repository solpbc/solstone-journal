// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, DurableEvent, append_durable_event};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::day_path;
use solstone_core_local::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_system::lifecycle::{
    DEFAULT_INTERVAL_SECONDS, ParentLossReason, ParentWatch, ParentWatchStatus,
    SupervisorLifecycle, SyncPeerObservation, SyncTickOutcome, sync_conflict_event,
    sync_peer_diagnostic,
};
use solstone_core_system::partition::canonical_journal_command;
use solstone_core_system::process::{ProcessInstanceSource, SystemProcessInstanceSource};
use solstone_core_system::process::{
    ProcessObservation as SystemProcessObservation, ProcessObservationTuple,
    classify_process_observation,
};
use solstone_core_system::provider_runtime::{
    CortexEventKind, CortexOutcomeEvent, LocalReadySideEffect, ProbeStatus, ProviderName,
    ProviderRetryState, ProviderRuntimeEvent, ProviderRuntimeEventSink, ProviderRuntimeNow,
    ProviderRuntimeState, ReasonCode, ReconcileContext, RuntimePhase, RuntimeStore,
    RuntimeStoreError, cancel_start, store_error_phase,
};
use solstone_core_system::queue::{SubmitOutcome, TaskQueue, TaskQueueStatusSnapshot};
use solstone_core_system::request::{
    BusTaskRequest, DailyCatchupProvenance, ExecutionRequest, TaskArgv,
};
use solstone_core_system::schedule::{ScheduleNow, ScheduleStatus};
use solstone_core_system::status_wire::{
    CrashedServiceCandidate, ProcessObservation as WireProcessObservation, ServiceCandidate,
    StaleHeartbeatWireInput, SupervisorStatusWireInput, project_supervisor_status,
};

use super::bus::{SupervisorProviderSink, SupervisorScheduleSink, emit};
use super::runtime::{
    AppExit, AppService, FlushState, ManagedAppProcess, RetainedSenseStatus, SupervisorState,
    apply_app_exit,
};
#[cfg(test)]
use super::{config, runtime};

const MAX_INBOUND_PER_TICK: usize = 256;
/// How often the supervisor reaps decoded-audio scratch a killed analysis or
/// merge left behind. With the transcribe crate's age floor, a leftover outlives
/// its writer by about two hours at most while the journal runs.
const SCRATCH_SWEEP_INTERVAL: Duration = Duration::from_secs(3600);

struct AppProcessSample {
    service: AppService,
    process_count: usize,
    tuple: Option<ProcessObservationTuple<i32>>,
}

#[allow(clippy::large_enum_variant)]
enum StatusEmissionPlan {
    Errors(Vec<&'static str>),
    Status(SupervisorStatusWireInput),
}

pub(crate) struct ShutdownSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_break: tokio::signal::windows::CtrlBreak,
    #[cfg(windows)]
    ctrl_close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    ctrl_logoff: tokio::signal::windows::CtrlLogoff,
    #[cfg(windows)]
    ctrl_shutdown: tokio::signal::windows::CtrlShutdown,
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
    /// Windows told the retained forwarder that this session is ending.
    ///
    /// The forwarder of an installed task inherits no upstream stop, so it
    /// latches its stop event for exactly one reason: a session-end broadcast.
    /// The OS terminates the whole tree a few seconds later whatever we do,
    /// which is why this takes the bounded shutdown rather than the standard
    /// budget -- markers left on disk are what an owner sees at the next
    /// logon.
    ///
    /// ⚠ Only the Windows path constructs this, but the variant is
    /// deliberately unconditional and carries an explicit dead-code allowance
    /// instead of a `cfg`. Gating it would push a `cfg(windows)` onto every
    /// match over the enum -- and, worse, onto the assertion that pins this
    /// reason to the bounded regime, which would then be compiled only on a
    /// platform where nothing in the rail *runs* this crate's lib tests. An
    /// unconditional variant keeps that assertion executing on Linux, where
    /// the canonical gate actually runs it.
    #[cfg_attr(not(windows), allow(dead_code))]
    HostSessionEnd,
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
        #[cfg(windows)]
        {
            // Logoff, shutdown and console close are the Windows session-end
            // events. tokio's handler deliberately never returns for those
            // three, which keeps this process alive while the standard
            // shutdown clears readiness, identity and children; the system
            // terminates the process after its own session-end grace.
            use tokio::signal::windows::{
                ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown,
            };
            Ok(Self {
                ctrl_c: ctrl_c().map_err(|error| error.to_string())?,
                ctrl_break: ctrl_break().map_err(|error| error.to_string())?,
                ctrl_close: ctrl_close().map_err(|error| error.to_string())?,
                ctrl_logoff: ctrl_logoff().map_err(|error| error.to_string())?,
                ctrl_shutdown: ctrl_shutdown().map_err(|error| error.to_string())?,
            })
        }
        #[cfg(not(any(unix, windows)))]
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
        #[cfg(windows)]
        tokio::select! {
            _ = self.ctrl_c.recv() => SupervisorSignal::SigInt,
            _ = self.ctrl_break.recv() => SupervisorSignal::SigInt,
            _ = self.ctrl_close.recv() => SupervisorSignal::SigTerm,
            _ = self.ctrl_logoff.recv() => SupervisorSignal::SigTerm,
            _ = self.ctrl_shutdown.recv() => SupervisorSignal::SigTerm,
        }
        #[cfg(not(any(unix, windows)))]
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
    retained_sense: Option<&'a RetainedSenseStatus>,
    now: Instant,
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
    let (sense_pending_queue_depth, sense_pending_age_ms, sense_pending_received) =
        match inputs.retained_sense {
            Some(sense) => {
                let age = inputs.now.saturating_duration_since(sense.received_at);
                (
                    Some(sense.pending_queue_depth as u64),
                    Some(age.as_millis() as u64),
                    true,
                )
            }
            None => (None, None, false),
        };
    StatusEmissionPlan::Status(SupervisorStatusWireInput {
        services,
        crashed,
        queue: inputs.queue,
        stale_heartbeats: inputs.stale_heartbeats,
        schedules: inputs.schedules,
        callosum_clients: inputs.callosum_clients,
        sense_pending_queue_depth,
        sense_pending_age_ms,
        sense_pending_received,
    })
}

/// Resolve once the retained forwarder latches its stop event, or once Windows
/// asks this process itself to end its session.
///
/// Polled rather than waited on a handle so the supervisor keeps one runtime
/// and no extra thread; the cadence bounds detection well inside the
/// forwarder's own session-end drain.
#[cfg(windows)]
async fn await_host_session_end(
    installed_task: Option<&solstone_core_system::process::AdmittedInstalledTaskLaunch>,
) {
    while !host_session_ending(installed_task) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The forwarder's stop event is the ordinary path. The supervisor's own
/// session-end window is the one that still works at a sign-out: Windows
/// ends the windowless children at once, and only a process it asks first
/// can clear the lifecycle markers.
#[cfg(windows)]
fn host_session_ending(
    installed_task: Option<&solstone_core_system::process::AdmittedInstalledTaskLaunch>,
) -> bool {
    installed_task.is_some_and(|task| task.stop_requested().unwrap_or(false))
        || solstone_core_system::process::windows_session_end_requested()
}

pub(crate) async fn run(
    state: &mut SupervisorState,
    lifecycle: &mut SupervisorLifecycle,
    shutdown: &mut ShutdownSignals,
    parent_watch: Option<ParentWatch>,
    #[cfg(windows)] installed_task: Option<
        &solstone_core_system::process::AdmittedInstalledTaskLaunch,
    >,
) -> SupervisorStopReason {
    let mut last_status = Instant::now() - state.timing.status_interval;
    let mut last_sync = Instant::now() - Duration::from_secs_f64(DEFAULT_INTERVAL_SECONDS);
    loop {
        if let Some(reason) =
            check_parent_watch(parent_watch.as_ref(), &SystemProcessInstanceSource)
        {
            return reason;
        }
        #[cfg(windows)]
        if host_session_ending(installed_task) {
            return SupervisorStopReason::HostSessionEnd;
        }
        #[cfg(windows)]
        let _ = solstone_core_system::process::observe_windows_launch_cleanup();
        let app_samples = reconcile_app_processes(state);
        let tick = Instant::now();
        solstone_core_system::processing::publish_display_snapshot(&state.journal);
        state.queue.resume_processing();
        state.queue.enforce_deadlines(tick);
        record_schedule_completions(state);
        reconcile_providers(state);
        if let Some(reason) = drain_inbound(state).await {
            return reason;
        }
        let wall = owner_now(&state.journal);
        check_segment_flush(&state.journal, &state.queue, &mut state.flush, None, tick);
        if scratch_sweep_due(state.last_scratch_sweep, tick) {
            sweep_abandoned_scratch(&state.journal);
            state.last_scratch_sweep = Some(tick);
        }
        let today = wall.format("%Y%m%d").to_string();
        check_index_todo(
            &state.journal,
            &state.queue,
            &mut state.last_index_todo_scan,
            tick,
            wall.timestamp_millis() as u64,
            Duration::from_secs(30),
        );
        check_activity_settle(
            &state.journal,
            &state.queue,
            &mut state.last_activity_settle_scan,
            tick,
            wall.timestamp_millis(),
            &today,
        );
        if !state.no_daily && state.daily.last_day != Some(wall.date_naive()) {
            if let Some(previous) = state.daily.last_day {
                check_segment_flush(
                    &state.journal,
                    &state.queue,
                    &mut state.flush,
                    Some(&previous.format("%Y%m%d").to_string()),
                    tick,
                );
            }
            state.daily.last_day = Some(wall.date_naive());
            state.discovery.request();
        }
        if let Some(scheduler) = state.scheduler.as_mut() {
            let schedule_sink = SupervisorScheduleSink {
                queue: state.queue.clone(),
                server: state.server.clone(),
            };
            let _ = scheduler.check(
                ScheduleNow::in_zone(wall.to_utc(), wall.timezone()),
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
            let wall = owner_now(&state.journal);
            let schedules = state
                .scheduler
                .as_ref()
                .map(|scheduler| {
                    scheduler.collect_status(ScheduleNow::in_zone(wall.to_utc(), wall.timezone()))
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
                retained_sense: state.retained_sense.as_ref(),
                now: status_now,
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
        #[cfg(windows)]
        tokio::select! {
            _ = tokio::time::sleep(state.timing.tick_interval) => {},
            signal = shutdown.wait() => return SupervisorStopReason::Signal(signal),
            () = await_host_session_end(installed_task) => {
                return SupervisorStopReason::HostSessionEnd;
            }
        }
        #[cfg(not(windows))]
        tokio::select! {
            _ = tokio::time::sleep(state.timing.tick_interval) => {},
            signal = shutdown.wait() => return SupervisorStopReason::Signal(signal),
        }
    }
}

fn scratch_sweep_due(last: Option<Instant>, tick: Instant) -> bool {
    last.is_none_or(|last| tick.saturating_duration_since(last) >= SCRATCH_SWEEP_INTERVAL)
}

pub(crate) fn check_index_todo(
    journal: &Path,
    queue: &TaskQueue,
    last_scan: &mut Option<Instant>,
    tick: Instant,
    now_ms: u64,
    interval: Duration,
) {
    if last_scan.is_some_and(|last| tick.saturating_duration_since(last) < interval) {
        return;
    }
    *last_scan = Some(tick);
    match solstone_core_indexer_store::todo::has_due(journal, now_ms) {
        Ok(true) if !queue.contains_reference("index-todo") => {
            submit_task(
                queue,
                canonical_journal_command(["indexer", "drain"]),
                "index-todo".into(),
                None,
                None,
            );
        }
        Ok(true) => {}
        Ok(false) => {}
        Err(error) => {
            log::warn!(target: "solstone::indexer", "could not check queued search work: {error}")
        }
    }
}

/// Reap the decoded audio a killed analysis or archive merge left outside its
/// own cleanup: shared temporary roots and the journal's merge working root.
fn sweep_abandoned_scratch(journal: &Path) {
    let analysis = solstone_core_transcribe::sweep_stale_analysis_scratch();
    let extractions =
        solstone_core_import_sources::archive::reap_abandoned_archive_extractions(journal);
    if analysis + extractions > 0 {
        log::info!(
            "supervisor: removed {analysis} stale analysis scratch dir(s) and {extractions} abandoned archive extraction(s)"
        );
    }
}

/// Flush each stream's last live segment once that stream has been idle for
/// the Python-compatible timeout, or at once when it belongs to `force_day`.
pub(crate) fn check_segment_flush(
    journal: &Path,
    queue: &TaskQueue,
    flush: &mut FlushState,
    force_day: Option<&str>,
    now: Instant,
) {
    solstone_core_think_cli::planner::check_segment_flush(journal, queue, flush, force_day, now);
}

/// Run `think --settle` for each stream whose activities are due a settle
/// check. The due times are kept on disk by the checks themselves, so a
/// restart loses none of them.
pub(crate) fn check_activity_settle(
    journal: &Path,
    queue: &TaskQueue,
    last_scan: &mut Instant,
    tick: Instant,
    now_ms: i64,
    today: &str,
) {
    solstone_core_think_cli::planner::check_activity_settle(
        journal, queue, last_scan, tick, now_ms, today,
    );
}

fn submit_task(
    queue: &TaskQueue,
    argv: Vec<String>,
    reference: String,
    day: Option<&str>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
) -> solstone_core_system::queue::SubmitOutcome {
    let cmd = TaskArgv::from_wire(argv).expect("supervisor constructs a non-empty argv");
    queue.submit(ExecutionRequest::Bus(BusTaskRequest {
        cmd,
        reference,
        day: day.map(str::to_owned),
        scheduler_name: None,
        queue_if_active_cmd_differs: false,
        priority: None,
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
            log::warn!(
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
                    log::warn!(
                        "supervisor: {} exited with {}; scheduling restart",
                        app.service.as_str(),
                        exit_code
                    );
                    apply_app_exit(app, &journal, AppExit::Process { code: *exit_code });
                }
                Ok(None) => {}
                Err(error) => {
                    log::warn!(
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
            log::warn!("supervisor: failed to record schedule completion for {name}: {error}");
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
    if let Some(exited) = state
        .local
        .shared
        .observe_exited_current_process(&state.local.processes, Instant::now())
    {
        state.local.coordinator.observe_process_exit(
            now,
            &mut state.local.state,
            &state.local.processes,
            &exited,
            &mut state.local.store,
            &mut local_sink,
        );
    }
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
    state.local.truth.reconcile_migration(&state.local.state);
    if let Some(managed) = state
        .local
        .truth
        .mlx_maintenance_request(&state.local.state, &state.local.processes)
        && !state.local.coordinator.request_local_maintenance(
            &mut state.local.state,
            &state.local.processes,
            &managed,
            &mut state.local.store,
        )
    {
        state.local.truth.cancel_mlx_maintenance_request();
    }
    for effect in state.local.store.take_ready_side_effects() {
        submit_local_ready_side_effect(&state.queue, effect);
    }
    #[cfg(windows)]
    if !synchronize_parakeet_sense_credentials(state) {
        return;
    }
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

/// Every processing surface -- the home page's attention banner, Thinking's
/// brain summary and its lane cards -- reads the persisted brain record, and
/// only a brain refresh writes it. A refresh that ran while Local was the lane
/// but its model was not yet installed or running -- a fresh journal before
/// local setup, or a daily check that caught the runtime stopped -- records
/// `blocked`, and nothing else re-checks once the runtime comes up. So a
/// runtime reaching Ready asks for one more refresh. The expected
/// fingerprint makes it a no-op when the brain is already ready or the active
/// lane has moved off this runtime; brain commands share one queue partition,
/// so it runs after any refresh already in flight rather than losing to it.
fn submit_local_ready_side_effect(queue: &TaskQueue, effect: LocalReadySideEffect) {
    let (argv, reference) = local_ready_task(effect);
    if submit_task(queue, argv, reference, None, None) == SubmitOutcome::Rejected {
        log::warn!("supervisor: local-ready brain refresh rejected");
    }
}

/// ⚠ `--expected-fingerprint` alone compares against the bundled runtime's
/// fingerprint, which is what the effect carries. Adding
/// `--expected-active-fingerprint` would compare it against the brain record's
/// own fingerprint instead, never match, and silently refresh nothing.
fn local_ready_task(effect: LocalReadySideEffect) -> (Vec<String>, String) {
    match effect {
        LocalReadySideEffect::RefreshBrain {
            expected_fingerprint_sha256,
        } => (
            canonical_journal_command([
                "brain",
                "refresh",
                "--expected-fingerprint",
                &expected_fingerprint_sha256,
            ]),
            format!("brain-refresh:local-ready:{expected_fingerprint_sha256}"),
        ),
    }
}

/// A Windows Parakeet launch rotates its in-memory loopback credential. Sense
/// can spawn native transcription children, so replace its process tree before
/// reconciling a ready provider result. Queued analysis resolves the same private
/// capability at each installation-bound spawn through its bound runtime.
/// The port/runtime record never becomes readable as a credential source.
#[cfg(windows)]
fn synchronize_parakeet_sense_credentials(state: &mut SupervisorState) -> bool {
    // Shutdown reconciles provider cleanup while hosted apps are exiting.
    // A credential revision at that point must not replace or restart Sense.
    if state
        .shutdown_started
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return true;
    }
    let Some((revision, credentials)) = state.parakeet.shared.sense_child_environment() else {
        return true;
    };
    if revision == state.parakeet_sense_credentials_revision {
        return true;
    }

    let sense = state
        .app_processes
        .iter_mut()
        .find(|app| app.service == AppService::Sense)
        .expect("app process inventory is complete");
    if let Some(process) = sense.process.as_mut() {
        if let Err(error) = process.terminate(Duration::from_secs(5)) {
            log::warn!(
                "supervisor: failed to replace Sense for Parakeet credential rotation: {error}"
            );
            return false;
        }
        process.cleanup();
    }
    sense.process = None;
    sense.started_at = None;
    sense.restart_at = Some(Instant::now());
    sense.backoff = None;

    state
        .sense_child_environment
        .environment
        .remove(&std::ffi::OsString::from("SOLSTONE_PARAKEET_AUTH_TOKEN"));
    state
        .sense_child_environment
        .environment
        .remove(&std::ffi::OsString::from("SOLSTONE_PARAKEET_AUTH_NONCE"));
    state
        .sense_child_environment
        .environment
        .remove(&std::ffi::OsString::from("SOLSTONE_PARAKEET_AUTH_PORT"));
    if let Some(credentials) = credentials {
        state
            .sense_child_environment
            .environment
            .extend(credentials);
    }
    state.parakeet_sense_credentials_revision = revision;
    false
}

async fn drain_inbound(state: &mut SupervisorState) -> Option<SupervisorStopReason> {
    for _ in 0..MAX_INBOUND_PER_TICK {
        let message =
            match tokio::time::timeout(Duration::ZERO, state.connection.next_message()).await {
                Ok(Some(message)) => message,
                _ => break,
            };
        #[cfg(windows)]
        if service_stop_is_admitted(state, &message) {
            return Some(SupervisorStopReason::Signal(SupervisorSignal::SigTerm));
        }
        handle_message(state, message);
    }
    None
}

/// This handler is reachable only through the existing authenticated Callosum
/// transport. Guard metadata cannot itself authenticate a process or borrow a generation.
#[cfg(windows)]
fn service_stop_is_admitted(state: &SupervisorState, message: &CallosumEnvelope) -> bool {
    use solstone_core_installation_identity::{
        GuardFields, journal_token_from_path, load_installation_binding, owner_base,
        parse_service_guard_environment, root_token_from_path,
    };
    use solstone_core_system::process::{InstanceVerdict, ProcessInstance};
    if message.tract != "supervisor" || message.event != "service_stop" {
        return false;
    }
    let Some(target) = message
        .extra
        .get("target")
        .cloned()
        .and_then(|value| serde_json::from_value::<ProcessInstance>(value).ok())
    else {
        return false;
    };
    if target.pid != std::process::id() || target.birth.windows_filetime().is_none() {
        return false;
    }
    if !matches!(
        SystemProcessInstanceSource.observe(&target),
        InstanceVerdict::SameLive { .. }
    ) {
        return false;
    }
    let Some(guard_object) = message.extra.get("guard").and_then(Value::as_object) else {
        return false;
    };
    let Some(environment) = guard_object
        .iter()
        .map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
        .collect::<Option<std::collections::BTreeMap<_, _>>>()
    else {
        return false;
    };
    if environment.len() != 4 {
        return false;
    }
    let Ok(Some(guard)) = parse_service_guard_environment(&environment) else {
        return false;
    };
    if guard != state.service_guard {
        return false;
    }
    // Require both boot-admitted binding and its current on-disk authority;
    // an old supervisor cannot be stopped using another installation/generation.
    let loaded = (|| {
        let owner = owner_base().ok()?;
        let root = crate::installation_context::identity_root_from_current_executable().ok()?;
        let binding = load_installation_binding(&owner, &root_token_from_path(&root).ok()?).ok()?;
        if binding.journal_token != journal_token_from_path(&state.journal).ok()? {
            return None;
        }
        Some(GuardFields::from_binding(&binding))
    })();
    loaded.as_ref() == Some(&guard)
}

fn handle_message(state: &mut SupervisorState, message: CallosumEnvelope) {
    handle_supervisor_request(state, &message);
    handle_supervisor_drain(state, &message);
    handle_segment_observed(state, &message);
    handle_sense_status(&mut state.retained_sense, &message);
    handle_think_daily_complete(state, &message);
    handle_segment_event_log(&state.journal, &message);
    handle_cortex_outcome(state, &message);
}

fn handle_sense_status(retained: &mut Option<RetainedSenseStatus>, message: &CallosumEnvelope) {
    if message.tract != "observe" || message.event != "status" {
        return;
    }
    if let Some(depth) = message
        .extra
        .get("pending_queue_depth")
        .and_then(Value::as_u64)
    {
        *retained = Some(RetainedSenseStatus {
            pending_queue_depth: depth as usize,
            received_at: Instant::now(),
        });
    }
}

fn decode_supervisor_request(message: &CallosumEnvelope) -> Result<BusTaskRequest, String> {
    let wire = serde_json::from_value::<solstone_core_system::request::WireTaskRequest>(
        Value::Object(message.extra.clone()),
    )
    .map_err(|error| error.to_string())?;
    BusTaskRequest::decode(wire, "native-supervisor").map_err(|error| error.to_string())
}

fn handle_supervisor_request(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "supervisor" || message.event != "request" {
        return;
    }
    let request = match decode_supervisor_request(message) {
        Ok(request) => request,
        Err(error) => {
            log::warn!("supervisor: {error}");
            return;
        }
    };
    if state.queue.submit(ExecutionRequest::Bus(request)) == SubmitOutcome::Rejected {
        log::warn!("supervisor: request rejected");
    }
}

fn handle_supervisor_drain(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "supervisor" || message.event != "drain" {
        return;
    }
    state.discovery.request();
}

/// Now in the journal's owner zone: the one clock every day, rollover and
/// schedule decision in the supervisor reads.
pub(crate) fn owner_now(journal: &std::path::Path) -> chrono::DateTime<chrono_tz::Tz> {
    chrono::Utc::now().with_timezone(&solstone_core_journal_config::owner_zone(journal))
}

fn handle_segment_observed(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "observe" || message.event != "observed" {
        return;
    }
    let Some(segment) = message_string(message, "segment") else {
        log::warn!("supervisor: observed message missing segment");
        return;
    };
    let day = message_string(message, "day")
        .map(str::to_owned)
        .unwrap_or_else(|| owner_now(&state.journal).format("%Y%m%d").to_string());
    let batch = message_truthy(message, "batch");
    let stream = message_string(message, "stream");
    solstone_core_think_cli::planner::plan_observed_segment(
        &state.journal,
        &state.queue,
        &mut state.flush,
        &day,
        segment,
        stream,
        batch,
    );
}

fn handle_think_daily_complete(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "think" || message.event != "daily_complete" {
        return;
    }
    let heartbeat_pid = state.journal.join("health/heartbeat.pid");
    #[cfg(unix)]
    if let Ok(contents) = std::fs::read_to_string(&heartbeat_pid)
        && let Ok(pid) = contents.trim().parse::<i32>()
    {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) | Err(nix::errno::Errno::EPERM) => {
                log::debug!("supervisor: heartbeat already running with pid {pid}");
                return;
            }
            Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => log::warn!("supervisor: could not check heartbeat pid {pid}: {error}"),
        }
    }
    #[cfg(windows)]
    if let Ok(contents) = std::fs::read_to_string(&heartbeat_pid)
        && let Ok(pid) = contents.trim().parse::<u32>()
    {
        match crate::heartbeat_pid_windows::recorded_pid_may_be_running(pid) {
            Ok(true) => {
                log::debug!("supervisor: heartbeat already running with pid {pid}");
                return;
            }
            Ok(false) => {}
            Err(error) => {
                log::warn!("supervisor: could not check heartbeat pid {pid}: {error}");
                return;
            }
        }
    }
    let _ = submit_task(
        &state.queue,
        canonical_journal_command(["heartbeat"]),
        "supervisor-heartbeat".to_owned(),
        None,
        None,
    );
}

fn handle_segment_event_log(journal: &Path, message: &CallosumEnvelope) {
    if !matches!(message.tract.as_str(), "observe" | "think") {
        return;
    }
    let (Some(day), Some(segment)) = (
        message_string(message, "day"),
        message_string(message, "segment"),
    ) else {
        return;
    };
    let day_dir = match day_path(journal, day, false) {
        Ok(path) => path,
        Err(error) => {
            log::warn!("supervisor: could not resolve event-log day {day}: {error}");
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
    let result = append_durable_event(&segment_dir, &DurableEvent::Callosum(message.clone()));
    if let Err(error) = result {
        log::warn!("supervisor: failed to append segment event log: {error}");
    }
}

fn handle_cortex_outcome(state: &mut SupervisorState, message: &CallosumEnvelope) {
    if message.tract != "cortex" || !matches!(message.event.as_str(), "start" | "finish" | "error")
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
        log::debug!("supervisor: local wedge recycle deferred; local service port unavailable");
        return;
    };
    if !local_probe_is_ready(state) {
        log::debug!("supervisor: local wedge recycle deferred; local health is not ready");
        return;
    }
    if let Err(error) = request_local_provider_recycle(state, failure_use_ids, port) {
        log::warn!("supervisor: local wedge recycle request failed: {error:?}");
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
            log::warn!("supervisor: could not read local endpoint configuration: {error}");
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
    // The supervisor fixture uses a pinned test binary; its probe is a test-only
    // stand-in for a healthy endpoint. Production uses the real ConnectOutcome.
    state.local.fixture_probe_ready
        || state.local.probe.probe_now(&state.local.state).status == ProbeStatus::Ready
}

fn request_local_provider_recycle(
    state: &mut SupervisorState,
    mut failure_use_ids: Vec<String>,
    port: u16,
) -> Result<(), RuntimeStoreError> {
    state.local.store.revoke_ready();
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
            log::error!("supervisor: sync conflict");
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
            log::error!("supervisor: sync renewal failure");
            log::error!("supervisor: sync renewal failure detail: {error:?}");
        }
        SyncTickOutcome::CompleteScanFailure(error) => {
            log::error!("supervisor: sync complete scan failure");
            log::error!("supervisor: sync complete scan failure detail: {error:?}");
        }
        SyncTickOutcome::RetainedObservationFailure(error) => {
            log::error!("supervisor: sync retained observation failure");
            log::error!("supervisor: sync retained observation failure detail: {error:?}");
        }
        SyncTickOutcome::StaleHeartbeatCollectionFailure(error) => {
            log::error!("supervisor: stale heartbeat collection failure");
            log::error!("supervisor: stale heartbeat collection failure detail: {error:?}");
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
    use solstone_core_callosum::{CallosumSocketConnection, CallosumSocketServer};
    use solstone_core_system::cap::{CapResolver, DefaultCapResolver};
    use solstone_core_system::lifecycle::{
        HeartbeatClassification, HeartbeatV2, RunId, SyncPeerIdentity, WriterId,
    };
    use solstone_core_system::partition::Partition;
    use solstone_core_system::process::{
        ExecutionState, InspectResult, InstanceCensus, ProcessBirth, ProcessInstance,
        ProcessInstanceSource,
    };
    use solstone_core_system::queue::{
        ProcessState, ProcessStateProbe, SystemProcessStateProbe, TaskQueue, TaskQueueOptions,
    };
    use tempfile::TempDir;

    use super::config::no_thinking_engine_chosen;
    use super::runtime::PendingFlush;
    use super::*;
    use solstone_core_think_cli::planner::{
        FLUSH_TIMEOUT, SETTLE_SCAN_INTERVAL, day_still_closing, flush_think_argv,
        is_mcp_audit_segment, settle_think_argv, submit_think,
    };

    #[test]
    fn scratch_sweep_runs_on_the_first_tick_and_then_hourly() {
        let origin = Instant::now();
        assert!(scratch_sweep_due(None, origin));
        assert!(!scratch_sweep_due(
            Some(origin),
            origin + SCRATCH_SWEEP_INTERVAL - Duration::from_secs(1)
        ));
        assert!(scratch_sweep_due(
            Some(origin),
            origin + SCRATCH_SWEEP_INTERVAL
        ));
    }

    #[test]
    fn an_mcp_audit_segment_is_not_submitted_for_enrichment() {
        // The envelope below is byte-for-byte the shape
        // `solstone-core-mcp-endpoint`'s `emit_observed` publishes, so this
        // pins the cross-crate contract rather than restating the constant.
        let audit: CallosumEnvelope = serde_json::from_value(json!({
            "tract": "observe", "event": "observed", "day": "20260831",
            "stream": "mcp.agent", "segment": "123456_1"
        }))
        .unwrap();
        assert!(is_mcp_audit_segment(message_string(&audit, "stream")));

        // The control that makes the negative mean something: an ordinary
        // capture segment still reaches enrichment.
        let capture: CallosumEnvelope = serde_json::from_value(json!({
            "tract": "observe", "event": "observed", "day": "20260831",
            "stream": "device", "segment": "120000_60"
        }))
        .unwrap();
        assert!(!is_mcp_audit_segment(message_string(&capture, "stream")));
        // A direct-layout segment carries no stream at all and is not audit.
        assert!(!is_mcp_audit_segment(None));
    }

    /// A `SupervisorState` built with a `TaskQueue` in `ready: false` mode: a
    /// call to `submit` records a pending reference and returns without ever
    /// reaching `start_dispatch`, so this drives the real handler — not a
    /// stand-in for it — without spawning a `journal think` child process.
    ///
    /// ⚠ Unix-only, because `supervisor::test_support` is `cfg(all(test,
    /// unix))` and this builds a literal `SupervisorState`, whose field set
    /// differs on Windows. Without the gate `cargo test -p solstone-core --lib
    /// --features test-hooks` does not compile for `x86_64-pc-windows-msvc` at
    /// all -- which is how an ungated version of it reached the default branch.
    /// The workspace Windows cross-check runs on Linux but *excludes* this
    /// crate: `solstone-core` reaches `ring`, `libsqlite3-sys` and
    /// `ffmpeg-sys-next`, three roots a Linux host cannot build. The native
    /// Windows gate now compiles this exact subject, so a repeat is caught per
    /// change rather than once per Windows package.
    #[cfg(unix)]
    async fn queue_only_state(journal: &std::path::Path) -> SupervisorState {
        fs::create_dir_all(journal.join("config")).expect("config dir");
        fs::write(
            journal.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"anthropic"}}}"#,
        )
        .expect("journal config writes");

        let socket_path = journal.join("callosum.sock");
        let server = Arc::new(
            CallosumSocketServer::bind(&socket_path)
                .await
                .expect("callosum server"),
        );
        let connection = CallosumSocketConnection::new(&socket_path, Map::new());
        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.to_path_buf(),
            cap_resolver: Arc::new(DefaultCapResolver::new(Duration::from_secs(1))),
            process_state_probe: Arc::new(SystemProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: false,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        let (local, parakeet) = super::super::test_support::stopped_providers(journal);
        SupervisorState {
            discovery: super::super::WorkDiscovery::new(
                journal.to_path_buf(),
                queue.clone(),
                false,
            ),
            journal: journal.to_path_buf(),
            no_daily: true,
            server,
            connection,
            queue,
            last_sync_snapshot: None,
            stale_heartbeats: Vec::new(),
            shutdown_started: std::sync::atomic::AtomicBool::new(false),
            started: Instant::now(),
            scheduler: None,
            recorded_schedule_completions: std::collections::BTreeSet::new(),
            app_processes: Vec::new(),
            local,
            parakeet,
            flush: FlushState::default(),
            daily: super::super::runtime::DailyState { last_day: None },
            last_activity_settle_scan: Instant::now() - Duration::from_secs(60),
            last_scratch_sweep: None,
            last_index_todo_scan: None,
            wedge: solstone_core_system::provider_runtime::WedgeState::default(),
            timing: super::super::runtime::SupervisorTiming {
                tick_interval: Duration::from_secs(1),
                status_interval: Duration::from_secs(5),
            },
            parent_loss_coordinator: None,
            sense_child_environment: solstone_core_system::process::ChildLaunchContext::default(),
            retained_sense: None,
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn index_work_is_found_without_a_bus_and_repeated_ticks_merge_one_drain() {
        use solstone_core_indexer_store::todo::enqueue_path;
        let journal = TempDir::new().unwrap();
        let state = queue_only_state(journal.path()).await;
        enqueue_path(journal.path(), Path::new("imports/one/summary.md")).unwrap();
        let origin = Instant::now();
        let mut last = None;
        check_index_todo(
            journal.path(),
            &state.queue,
            &mut last,
            origin,
            u64::MAX,
            Duration::from_secs(30),
        );
        check_index_todo(
            journal.path(),
            &state.queue,
            &mut last,
            origin + Duration::from_secs(30),
            u64::MAX,
            Duration::from_secs(30),
        );
        assert!(state.queue.contains_reference("index-todo"));
        assert_eq!(
            state.queue.collect_queue_counts().values().sum::<usize>(),
            1
        );

        let delayed = TempDir::new().unwrap();
        let delayed_state = queue_only_state(delayed.path()).await;
        enqueue_path(delayed.path(), Path::new("imports/later/summary.md")).unwrap();
        let path = fs::read_dir(delayed.path().join("indexer/todo"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut entry: solstone_core_indexer_store::todo::Entry =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        entry.not_before_ms = u64::MAX;
        fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
        check_index_todo(
            delayed.path(),
            &delayed_state.queue,
            &mut None,
            origin,
            1,
            Duration::from_secs(30),
        );
        assert!(!delayed_state.queue.contains_reference("index-todo"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_guard_is_load_bearing_removing_it_would_submit_the_audit_segment() {
        // ⚠ The predicate test above pins `is_mcp_audit_segment` in isolation
        // — this drives `handle_segment_observed` itself, against a queue
        // that records a submission without dispatching one. The bar this
        // meets: delete the `is_mcp_audit_segment` check from the handler and
        // the first assertion below goes red, because the audit segment's
        // reference would then land in the queue exactly like the control's.
        let journal = TempDir::new().expect("temporary journal");
        let mut state = queue_only_state(journal.path()).await;

        let audit: CallosumEnvelope = serde_json::from_value(json!({
            "tract": "observe", "event": "observed", "day": "20260831",
            "stream": "mcp.agent", "segment": "123456_1"
        }))
        .unwrap();
        handle_segment_observed(&mut state, &audit);
        assert!(
            !state
                .queue
                .contains_reference("supervisor-observed-20260831-123456_1"),
            "an mcp audit segment must not be submitted for enrichment"
        );

        // The control that makes the negative mean something: an ordinary
        // capture segment, observed by the same handler against the same
        // state, still reaches the queue.
        let capture: CallosumEnvelope = serde_json::from_value(json!({
            "tract": "observe", "event": "observed", "day": "20260831",
            "stream": "device", "segment": "120000_60"
        }))
        .unwrap();
        handle_segment_observed(&mut state, &capture);
        assert!(
            state
                .queue
                .contains_reference("supervisor-observed-20260831-120000_60"),
            "an ordinary capture segment must still be submitted for enrichment"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn each_stream_keeps_its_own_last_segment_for_the_idle_flush() {
        let journal = TempDir::new().expect("temporary journal");
        let mut state = queue_only_state(journal.path()).await;
        for (stream, segment) in [
            ("desktop", "090000_300"),
            ("phone", "090500_300"),
            ("phone", "091000_300"),
        ] {
            let observed: CallosumEnvelope = serde_json::from_value(json!({
                "tract": "observe", "event": "observed", "day": "20260831",
                "stream": stream, "segment": segment
            }))
            .unwrap();
            handle_segment_observed(&mut state, &observed);
        }
        let last = state
            .flush
            .pending
            .iter()
            .map(|(stream, pending)| (stream.as_deref(), pending.segment.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            last,
            [
                (Some("desktop"), "090000_300"),
                (Some("phone"), "091000_300")
            ]
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn late_segments_are_thought_live_without_replacing_the_latest_idle_flush() {
        let journal = TempDir::new().expect("temporary journal");
        let mut state = queue_only_state(journal.path()).await;
        for (day, segment) in [
            ("20260831", "100500_300"),
            ("20260831", "090000_300"),
            ("20260830", "230000_300"),
        ] {
            let observed: CallosumEnvelope = serde_json::from_value(json!({
                "tract": "observe", "event": "observed", "day": day,
                "stream": "desktop", "segment": segment
            }))
            .unwrap();
            handle_segment_observed(&mut state, &observed);
            assert!(
                state
                    .queue
                    .contains_reference(&format!("supervisor-observed-{day}-{segment}")),
                "late segments must still be thought"
            );
        }
        let pending = state
            .flush
            .pending
            .get(&Some("desktop".to_owned()))
            .unwrap();
        assert_eq!(
            (&pending.day[..], &pending.segment[..]),
            ("20260831", "100500_300")
        );
        let due = pending.last_segment_ts + FLUSH_TIMEOUT;
        check_segment_flush(&state.journal, &state.queue, &mut state.flush, None, due);
        assert!(
            state
                .queue
                .contains_reference("supervisor-flush-20260831-100500_300")
        );
        assert!(state.flush.pending.is_empty());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_local_runtime_reaching_ready_queues_one_brain_refresh() {
        // The runtime store records a refresh request when Local publishes
        // Ready; the brain record every processing surface reads stays at a
        // stale `blocked` unless the supervisor actually submits it.
        use solstone_core_system::provider_runtime::{InFlight, ProviderFence, ReadyProcess};

        let journal = TempDir::new().expect("temporary journal");
        let mut state = queue_only_state(journal.path()).await;
        let reference = "brain-refresh:local-ready:fingerprint";

        reconcile_providers(&mut state);
        assert!(
            !state.queue.contains_reference(reference),
            "a runtime that never reached Ready asks for no refresh"
        );

        let fence = ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 4,
            fingerprint: Some("fingerprint".to_owned()),
            attempt: 1,
        };
        state.local.shared.record_ready_process(
            &fence,
            ReadyProcess {
                process_id: "local:42".to_owned(),
                process_name: "local".to_owned(),
                pid: 42,
                port: 4312,
            },
        );
        let mut ready = ProviderRuntimeState::new(ProviderName::Local);
        ready.generation = 4;
        ready.desired_fingerprint = Some("fingerprint".to_owned());
        ready.latest_phase = RuntimePhase::Ready;
        ready.retry.attempt_count = 1;
        ready.start = Some(InFlight {
            fence,
            result: None,
        });
        state
            .local
            .store
            .publish_state(&ready)
            .expect("ready publishes");

        reconcile_providers(&mut state);
        assert!(
            state.queue.contains_reference(reference),
            "a runtime reaching Ready must queue a brain refresh"
        );
        assert!(
            state.local.store.take_ready_side_effects().is_empty(),
            "the request is consumed, so a later reconcile cannot submit it again"
        );
    }

    #[test]
    fn the_local_ready_refresh_compares_against_the_runtime_fingerprint() {
        let (argv, reference) = local_ready_task(LocalReadySideEffect::RefreshBrain {
            expected_fingerprint_sha256: "fingerprint".to_owned(),
        });
        assert_eq!(
            argv,
            [
                "solstone",
                "journal",
                "brain",
                "refresh",
                "--expected-fingerprint",
                "fingerprint"
            ]
        );
        assert_eq!(reference, "brain-refresh:local-ready:fingerprint");
    }

    #[test]
    fn segment_event_log_keeps_concurrent_custody_rows_intact() {
        let bed = Bed::new("event-log-concurrent");
        let segment = bed.root.join("chronicle/20260804/device/120000_60");
        fs::create_dir_all(&segment).unwrap();
        let event: solstone_core_callosum::DeviceIngestEvent = serde_json::from_value(json!({
            "record_type": "device_ingest", "record_version": 1, "protocol_version": 3,
            "outcome": "accepted", "cid": "test-device", "source": "", "stream": "device",
            "day": "20260804", "segment": "120000_60", "files": [], "meta": {}
        }))
        .unwrap();
        let message: CallosumEnvelope = serde_json::from_value(json!({
            "tract": "observe", "event": "observed", "day": "20260804",
            "stream": "device", "segment": "120000_60", "detail": "x".repeat(8192)
        }))
        .unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..64 {
                    barrier.wait();
                    handle_segment_event_log(&bed.root, &message);
                }
            });
            scope.spawn(|| {
                for _ in 0..64 {
                    barrier.wait();
                    append_durable_event(&segment, &DurableEvent::DeviceIngest(event.clone()))
                        .unwrap();
                }
            });
        });
        let report = solstone_core_callosum::read_durable_events(&segment).unwrap();
        assert_eq!(report.unparseable, 0);
        assert_eq!(report.unrecognized, 0);
        assert_eq!(report.records.len(), 128);
        let receipts = solstone_core_callosum::read_device_ingest_events(&segment).unwrap();
        assert_eq!(receipts.records.len(), 64);
        assert_eq!(receipts.wrong_family, 64);
        let expected = serde_json::to_value(&event).unwrap();
        assert!(
            receipts
                .records
                .iter()
                .all(|receipt| serde_json::to_value(receipt).unwrap() == expected)
        );
    }

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    struct Bed {
        root: PathBuf,
    }

    impl Bed {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "solstone-supervisor-tick-{name}-{}-{}",
                std::process::id(),
                NEXT_PATH.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("temporary journal");
            fs::create_dir_all(root.join("config")).expect("config directory");
            Self { root }
        }

        fn enable_thinking(&self) {
            fs::write(
                self.root.join("config/journal.json"),
                br#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"local"}}}"#,
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: root.to_path_buf(),
            cap_resolver: Arc::new(FixedCap),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: false,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
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
            held: Vec::new(),
            queue_hold: None,
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
            retained_sense: None,
            now: Instant::now(),
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
    fn status_emission_plan_projects_queue_hold() {
        let local = provider_state(ProviderName::Local, RuntimePhase::Ready);
        let parakeet = provider_state(ProviderName::Parakeet, RuntimePhase::Stopped);
        let mut queue = empty_queue_snapshot();
        queue.queue_hold = Some(solstone_core_system::queue_hold::QueueHoldStatus {
            reason: solstone_core_system::queue_hold::QueueHoldReason::RecordsUnreadable,
            detail: "cannot read in-flight holds directory".to_owned(),
        });

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
            queue,
            stale_heartbeats: Vec::new(),
            schedules: Vec::new(),
            callosum_clients: 2,
            retained_sense: None,
            now: Instant::now(),
        });

        let StatusEmissionPlan::Status(input) = plan else {
            panic!("determinate observations must produce a status plan");
        };
        assert_eq!(
            input.queue.queue_hold,
            Some(solstone_core_system::queue_hold::QueueHoldStatus {
                reason: solstone_core_system::queue_hold::QueueHoldReason::RecordsUnreadable,
                detail: "cannot read in-flight holds directory".to_owned(),
            })
        );
    }

    #[test]
    fn status_emission_plan_projects_held_partition_from_real_task_queue() {
        let temp_dir = tempfile::tempdir().unwrap();
        let journal_root = temp_dir.path().to_path_buf();
        let scope_name = solstone_core_system::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &solstone_core_system::process::ProcessInstance {
                pid: 100,
                birth: solstone_core_system::process::ProcessBirth::linux(100, 0, 100),
            },
        );
        let scope_dir =
            solstone_core_system::queue_hold_store::scope_directory(&journal_root, &scope_name);
        std::fs::create_dir_all(&scope_dir).unwrap();

        let rec = solstone_core_system::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "hold-part-test".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["ref-held-1".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: 1000,
            created_unix: 1234567,
            root: None,
            group_id: None,
            bound: Vec::new(),
            exit_code: None,
            reasons: vec![solstone_core_system::queue_hold::ReasonCode::RootLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = solstone_core_system::queue_hold_store::partition_record_path(
            &journal_root,
            &scope_name,
            &solstone_core_system::partition::Partition::new("svc"),
        );
        solstone_core_system::queue_hold_store::write_in_flight_record(&rec_path, &rec).unwrap();

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal_root.clone(),
            cap_resolver: Arc::new(DefaultCapResolver::new(Duration::from_secs(1))),
            process_state_probe: Arc::new(SystemProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });

        let queue_snapshot = queue.collect_status_snapshot(Instant::now());
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
            queue: queue_snapshot,
            stale_heartbeats: Vec::new(),
            schedules: Vec::new(),
            callosum_clients: 2,
            retained_sense: None,
            now: Instant::now(),
        });

        let StatusEmissionPlan::Status(input) = plan else {
            panic!("determinate observations must produce a status plan");
        };
        assert_eq!(input.queue.held.len(), 1);
        assert_eq!(
            input.queue.held[0].partition,
            solstone_core_system::partition::Partition::new("svc")
        );
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
            retained_sense: None,
            now: Instant::now(),
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
            retained_sense: None,
            now: Instant::now(),
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
            retained_sense: None,
            now: Instant::now(),
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

    fn wall_time(day: u32, seconds: u64) -> SystemTime {
        let midnight = date(day).and_hms_opt(0, 0, 0).unwrap().and_utc();
        SystemTime::from(midnight) + Duration::from_secs(seconds)
    }

    fn pending_flush(at: Instant, day: &str, segment: &str) -> PendingFlush {
        PendingFlush {
            last_segment_ts: at,
            day: day.to_owned(),
            segment: segment.to_owned(),
        }
    }

    #[test]
    fn a_stream_whose_settle_check_is_due_gets_one_and_a_restart_keeps_it() {
        let bed = Bed::new("settle-due");
        bed.enable_thinking();
        let queue = queue(&bed.root);
        let dir = bed.root.join("awareness/activity_settle");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("watch.json"),
            r#"{"due_ms":1000,"days":{"20260101":{}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("desktop.json"),
            r#"{"due_ms":5000,"days":{"20260101":{}}}"#,
        )
        .unwrap();
        let origin = Instant::now();
        let mut last = origin - SETTLE_SCAN_INTERVAL;

        check_activity_settle(&bed.root, &queue, &mut last, origin, 2000, "20260101");
        assert_eq!(pending(&queue), 1, "only the stream that is due");
        assert!(queue.contains_reference("supervisor-settle-watch"));
        assert_eq!(
            settle_think_argv("watch"),
            [
                "solstone", "journal", "think", "-v", "--settle", "--stream", "watch"
            ]
            .map(str::to_owned)
        );

        // Within the scan interval nothing is read again; once it has
        // passed, a check already queued is not queued twice.
        check_activity_settle(&bed.root, &queue, &mut last, origin, 9000, "20260101");
        assert_eq!(pending(&queue), 1);
        let later = origin + SETTLE_SCAN_INTERVAL;
        check_activity_settle(&bed.root, &queue, &mut last, later, 9000, "20260101");
        assert_eq!(pending(&queue), 2);
        assert!(queue.contains_reference("supervisor-settle-desktop"));

        // A new supervisor reads the same due times from disk.
        let restarted = self::queue(&bed.root);
        let mut fresh = later - SETTLE_SCAN_INTERVAL;
        check_activity_settle(&bed.root, &restarted, &mut fresh, later, 9000, "20260101");
        assert_eq!(pending(&restarted), 2);
        assert!(restarted.contains_reference("supervisor-settle-watch"));
    }

    #[test]
    fn check_segment_flush_forces_expected_command_and_clears_the_stream() {
        let bed = Bed::new("forced-flush");
        bed.enable_thinking();
        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut flush = FlushState::default();
        flush.pending.insert(
            Some("camera".to_owned()),
            pending_flush(origin, "20260101", "120000_1"),
        );

        check_segment_flush(&bed.root, &queue, &mut flush, Some("20260101"), origin);

        assert!(flush.pending.is_empty());
        assert_eq!(pending(&queue), 1);
        assert_eq!(
            flush_think_argv("20260101", "120000_1", Some("camera")),
            [
                "solstone",
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

        flush.pending.insert(
            Some("camera".to_owned()),
            pending_flush(origin, "20260101", "120000_1"),
        );
        check_segment_flush(
            &bed.root,
            &queue,
            &mut flush,
            None,
            origin + FLUSH_TIMEOUT - Duration::from_secs(1),
        );
        assert_eq!(flush.pending.len(), 1);
        assert_eq!(pending(&queue), 1);

        check_segment_flush(&bed.root, &queue, &mut flush, None, origin + FLUSH_TIMEOUT);
        assert!(flush.pending.is_empty());
        assert_eq!(pending(&queue), 2);
    }

    #[test]
    fn a_stream_that_keeps_sending_does_not_hold_back_another_streams_flush() {
        let bed = Bed::new("per-stream-flush");
        bed.enable_thinking();
        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut flush = FlushState::default();
        flush.pending.insert(
            Some("desktop".to_owned()),
            pending_flush(origin, "20260101", "090000_300"),
        );
        // The phone's newest segment is recent, the desktop's is an hour old.
        flush.pending.insert(
            Some("phone".to_owned()),
            pending_flush(origin + FLUSH_TIMEOUT, "20260101", "100000_300"),
        );

        check_segment_flush(&bed.root, &queue, &mut flush, None, origin + FLUSH_TIMEOUT);

        assert_eq!(pending(&queue), 1);
        assert_eq!(
            flush.pending.keys().cloned().collect::<Vec<_>>(),
            [Some("phone".to_owned())]
        );
    }

    #[test]
    fn a_day_that_just_ended_runs_once_its_last_inputs_are_in() {
        let bed = Bed::new("day-still-closing");
        bed.enable_thinking();
        bed.updated_day("20260106");
        let settle = bed.root.join("awareness/activity_settle");
        let runs = |held: Option<&str>, seconds: u64| {
            let queue = queue(&bed.root);
            if let Some(reference) = held {
                let _ = submit_think(
                    &queue,
                    canonical_journal_command(["think", "--day", "20260106"]),
                    "20260106",
                    reference.to_owned(),
                );
            }
            day_still_closing(&bed.root, &queue, wall_time(7, seconds)).is_none()
        };
        // A segment that began before midnight may still be on its way.
        assert!(!runs(None, 9 * 60));
        // Its flush, a segment or activity of it, or a settle check still
        // running holds it.
        for reference in [
            "supervisor-flush-20260106-235500_300",
            "supervisor-observed-20260106-235800_300",
            "supervisor-settle-desktop",
            r#"supervisor-activity-{"day":"20260106","facet":"work","activity":"a"}"#,
        ] {
            assert!(!runs(Some(reference), 11 * 60), "{reference}");
        }
        // Work for today does not.
        assert!(runs(
            Some("supervisor-observed-20260107-000500_300"),
            11 * 60
        ));
        // An activity of it still waiting to be written, or whose talents
        // are still running, holds it.
        let publishing = settle.join("publishing");
        fs::create_dir_all(&publishing).expect("publishing dir");
        fs::write(
            publishing.join("desktop-1-1.json"),
            json!({"days": ["20260106"]}).to_string(),
        )
        .expect("publishing");
        assert!(!runs(None, 11 * 60));
        fs::remove_dir_all(&publishing).expect("published");
        fs::create_dir_all(&settle).expect("settle dir");
        fs::write(
            settle.join("desktop.json"),
            json!({"due_ms": 0, "days": {"20260106": {}}}).to_string(),
        )
        .expect("settle state");
        assert!(!runs(None, 11 * 60));
        // An hour after the day ends it runs whatever is still pending.
        assert!(runs(None, 60 * 60));
        // Once nothing of it is left, it runs.
        fs::remove_file(settle.join("desktop.json")).expect("settled");
        assert!(runs(Some("supervisor-catchup-20260105"), 11 * 60));
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
    fn supervisor_wire_boundary_rejects_malformed_commands_and_priority() {
        let mut missing = request_with_cmd(json!([]));
        missing.extra.clear();
        assert!(decode_supervisor_request(&missing).is_err());
        for cmd in [json!("journal"), json!([1, "brain"]), json!([])] {
            assert!(decode_supervisor_request(&request_with_cmd(cmd)).is_err());
        }
        for priority in [json!({"band":"wrong","source_time":1}), json!("4:1")] {
            let mut message = request_with_cmd(json!(["journal", "think"]));
            message.extra.insert("priority".into(), priority);
            assert!(decode_supervisor_request(&message).is_err());
        }
    }

    #[test]
    fn supervisor_wire_boundary_retains_priority_and_never_accepts_internal_provenance() {
        let mut message = request_with_cmd(json!(["journal", "think", "--day", "20260101"]));
        message.extra.insert(
            "priority".into(),
            json!({"band":"import","source_time":20260101120000u64}),
        );
        message
            .extra
            .insert("daily_catchup_provenance".into(), json!({"day":"20260101"}));
        let request = decode_supervisor_request(&message).unwrap();
        assert_eq!(request.priority.unwrap().to_string(), "4:20260101120000");
        assert!(request.daily_catchup_provenance.is_none());
        assert_eq!(request.reference, "native-supervisor");
        assert!(matches!(
            decode_supervisor_request(&request_with_cmd(json!(["journal", "brain", "refresh"])))
                .unwrap()
                .cmd,
            TaskArgv::Brain(_)
        ));
        assert!(matches!(
            decode_supervisor_request(&request_with_cmd(json!([
                "/opt/sol/solstone-core-journal",
                "brain",
                "refresh"
            ])))
            .unwrap()
            .cmd,
            TaskArgv::Unknown { .. }
        ));
    }

    #[test]
    fn no_thinking_engine_predicates_agree_on_absent_empty_and_unreadable_provider() {
        let bed = Bed::new("no-engine-predicates");
        fs::create_dir_all(bed.root.join("config")).expect("config dir");

        fs::write(
            bed.root.join("config/journal.json"),
            br#"{"processing":{"mode":"immediate"}}"#,
        )
        .expect("write absent providers");
        let sense_config_a = solstone_core_sense::config::read_config(&bed.root);
        assert!(no_thinking_engine_chosen(&bed.root));
        assert!(solstone_core_sense::config::no_thinking_engine(
            &sense_config_a
        ));

        fs::write(
            bed.root.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"   "}}}"#,
        )
        .expect("write empty provider");
        let sense_config_b = solstone_core_sense::config::read_config(&bed.root);
        assert!(no_thinking_engine_chosen(&bed.root));
        assert!(solstone_core_sense::config::no_thinking_engine(
            &sense_config_b
        ));

        fs::write(bed.root.join("config/journal.json"), b"{").expect("write unreadable config");
        let sense_config_c = solstone_core_sense::config::read_config(&bed.root);
        assert!(no_thinking_engine_chosen(&bed.root));
        assert!(solstone_core_sense::config::no_thinking_engine(
            &sense_config_c
        ));

        fs::write(
            bed.root.join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"local"}}}"#,
        )
        .expect("write valid provider");
        let sense_config_d = solstone_core_sense::config::read_config(&bed.root);
        assert!(!no_thinking_engine_chosen(&bed.root));
        assert!(!solstone_core_sense::config::no_thinking_engine(
            &sense_config_d
        ));
    }

    #[test]
    fn handle_sense_status_updates_retained_state() {
        let mut retained_sense = None;
        let message = CallosumEnvelope {
            tract: "observe".to_owned(),
            event: "status".to_owned(),
            ts: None,
            extra: serde_json::Map::from_iter([(
                "pending_queue_depth".to_owned(),
                serde_json::json!(5),
            )]),
        };

        handle_sense_status(&mut retained_sense, &message);
        assert!(retained_sense.is_some());
        let retained = retained_sense.unwrap();
        assert_eq!(retained.pending_queue_depth, 5);
    }

    #[test]
    fn plan_status_emission_projects_retained_sense_depth_and_age() {
        let local = provider_state(ProviderName::Local, RuntimePhase::Ready);
        let parakeet = provider_state(ProviderName::Parakeet, RuntimePhase::Ready);
        let received_at = Instant::now();
        let now = received_at + Duration::from_secs(3);
        let sense = RetainedSenseStatus {
            pending_queue_depth: 7,
            received_at,
        };

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
            retained_sense: Some(&sense),
            now,
        });

        let StatusEmissionPlan::Status(input) = plan else {
            panic!("expected status");
        };
        assert_eq!(input.sense_pending_queue_depth, Some(7));
        assert_eq!(input.sense_pending_age_ms, Some(3000));
        assert!(input.sense_pending_received);
    }
}
