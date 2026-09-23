// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
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
use solstone_core_system::request::{
    BusTaskRequest, DailyCatchupProvenance, ExecutionRequest, TaskArgv,
};
use solstone_core_system::schedule::{ScheduleNow, ScheduleStatus};
use solstone_core_system::status_wire::{
    CrashedServiceCandidate, ProcessObservation as WireProcessObservation, ServiceCandidate,
    StaleHeartbeatWireInput, SupervisorStatusWireInput, project_supervisor_status,
};
use solstone_core_system::{
    catchup::{CatchupError, eligible_catchup_days, reconcile_stale_catchup_attempts},
    queue::{SubmitOutcome, TaskQueue, TaskQueueStatusSnapshot},
};

use super::bus::{SupervisorProviderSink, SupervisorScheduleSink, emit};
use super::config::{no_thinking_engine_chosen, processing_is_deferred};
use super::runtime::{
    AppExit, AppService, DailyState, FlushState, ManagedAppProcess, RetainedSenseStatus,
    SupervisorState, apply_app_exit,
};

const MAX_INBOUND_PER_TICK: usize = 256;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3600);
pub(crate) const RETRY_EXPIRY_INTERVAL: Duration = Duration::from_secs(60);
/// A retry-expiry drain runs on the tick loop's own thread; one this long is worth a log
/// line. Well above a healthy pass and well below the ~60 s at which the old throttle re-fired.
const SLOW_RETRY_EXPIRY_DRAIN: Duration = Duration::from_secs(30);

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
        state.queue.enforce_deadlines(tick);
        record_schedule_completions(state);
        reconcile_providers(state);
        if let Some(reason) = drain_inbound(state).await {
            return reason;
        }
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
        let today = wall.format("%Y%m%d").to_string();
        let (seed_outcome, drain_outcome) = activity_retry_drain_with(
            state.no_daily,
            state.is_remote_mode,
            processing_is_deferred(&state.journal),
            no_thinking_engine_chosen(&state.journal),
            &mut state.activity_retry_seed_day,
            &mut state.last_activity_retry_drain,
            &today,
            tick,
            || {
                let yesterday = (wall.date_naive() - chrono::Duration::days(1))
                    .format("%Y%m%d")
                    .to_string();
                [yesterday, today.clone()].iter().try_for_each(|day| {
                    solstone_core_think_cli::seed_activity_retries(
                        &state.journal,
                        day,
                        wall.timestamp_millis(),
                    )
                })
            },
            || run_activity_retry_drain(&state.journal, &state.queue, wall.timestamp_millis()),
        );
        if let Some(Err(error)) = seed_outcome {
            log::warn!("supervisor: activity retry recovery failed: {error}");
        }
        if let Some(Err(error)) = drain_outcome {
            log::warn!("supervisor: activity retry drain failed: {error}");
        }
        if !state.no_daily {
            let daily_outcome = compose_daily_and_retry_expiry_with(
                &mut state.last_retry_expiry_drain,
                tick,
                || {
                    handle_daily_tasks(
                        &state.journal,
                        &state.queue,
                        state.is_remote_mode,
                        &mut state.daily,
                        &mut state.flush,
                        wall.date_naive(),
                        wall_now,
                    )
                },
                |last_drain| {
                    handle_retry_expiry_drain(
                        state.is_remote_mode,
                        processing_is_deferred(&state.journal),
                        &state.journal,
                        &state.queue,
                        last_drain,
                        wall.date_naive(),
                        tick,
                        wall_now,
                    )
                },
            );
            match daily_outcome {
                Ok(()) => {}
                Err(DailyOrExpiryError::Daily(error)) => {
                    log::warn!("supervisor: daily catchup drain failed: {error}");
                }
                Err(DailyOrExpiryError::RetryExpiry(error)) => {
                    log::warn!("supervisor: retry-expiry catchup drain failed: {error}");
                }
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
        log::warn!("supervisor: daily state not initialized; skipping daily processing");
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
    retry_expiry_drain_with(is_remote, is_deferred, last_drain, today, tick, |exclude| {
        // A bounded persisted reconciliation also discovers derived-only edits and lost
        // notifications. The ordinary selector still applies pacing, current-day exclusion,
        // and the four-day cap.
        run_catchup_drain(journal, queue, exclude, &[], now)
    })
}

type ActivityRetryDrainOutcome = (Option<Result<(), String>>, Option<Result<(), String>>);

#[allow(clippy::too_many_arguments)] // The tick's clock/watermark seams remain explicitly injectable.
fn activity_retry_drain_with(
    no_daily: bool,
    is_remote_mode: bool,
    is_deferred: bool,
    no_engine: bool,
    seed_day: &mut Option<String>,
    last_drain: &mut Instant,
    today: &str,
    tick: Instant,
    seed: impl FnOnce() -> Result<(), String>,
    drain: impl FnOnce() -> Result<(), String>,
) -> ActivityRetryDrainOutcome {
    if no_daily
        || is_remote_mode
        || is_deferred
        || no_engine
        || tick.saturating_duration_since(*last_drain) < RETRY_EXPIRY_INTERVAL
    {
        return (None, None);
    }

    let seed_result = if seed_day.as_deref() != Some(today) {
        let res = seed();
        if res.is_ok() {
            *seed_day = Some(today.to_owned());
        }
        Some(res)
    } else {
        None
    };

    let drain_result = Some(drain());
    *last_drain = scan_finished(tick);
    (seed_result, drain_result)
}

#[derive(Debug)]
enum DailyOrExpiryError {
    Daily(CatchupError),
    RetryExpiry(CatchupError),
}

fn compose_daily_and_retry_expiry_with(
    last_retry_expiry_drain: &mut Instant,
    tick: Instant,
    rollover: impl FnOnce() -> Result<bool, CatchupError>,
    retry_expiry: impl FnOnce(&mut Instant) -> Result<(), CatchupError>,
) -> Result<(), DailyOrExpiryError> {
    match rollover() {
        Ok(true) => {
            *last_retry_expiry_drain = scan_finished(tick);
            Ok(())
        }
        Err(error) => {
            *last_retry_expiry_drain = scan_finished(tick);
            Err(DailyOrExpiryError::Daily(error))
        }
        Ok(false) => retry_expiry(last_retry_expiry_drain).map_err(DailyOrExpiryError::RetryExpiry),
    }
}

/// The retry-expiry gate and throttle around one catch-up scan. The throttle runs from when
/// the scan finished, on success or failure: a scan as long as the interval must not be
/// followed at once by another that re-reads the same days.
fn retry_expiry_drain_with(
    is_remote: bool,
    is_deferred: bool,
    last_drain: &mut Instant,
    today: chrono::NaiveDate,
    tick: Instant,
    scan: impl FnOnce(&BTreeSet<String>) -> Result<(), CatchupError>,
) -> Result<(), CatchupError> {
    if is_remote || is_deferred {
        return Ok(());
    }
    if tick.saturating_duration_since(*last_drain) < RETRY_EXPIRY_INTERVAL {
        return Ok(());
    }
    let exclude = BTreeSet::from([today.format("%Y%m%d").to_string()]);
    let started = Instant::now();
    let scanned = scan(&exclude);
    *last_drain = scan_finished(tick);
    let took = started.elapsed();
    if took >= SLOW_RETRY_EXPIRY_DRAIN {
        log::warn!(
            "supervisor: retry-expiry catchup drain blocked the tick for {:.1}s",
            took.as_secs_f64()
        );
    }
    scanned
}

/// The instant a catch-up scan that began at `tick` counts as having finished, for throttling.
/// `tick` is the loop-top instant, so a scan that took time ends later; a `tick` already in
/// the future (a synthetic tick) is kept.
fn scan_finished(tick: Instant) -> Instant {
    tick.max(Instant::now())
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
    let reconcile_res = reconcile(journal, now);
    let today_str = today.format("%Y%m%d").to_string();
    run_today_sense_repair(journal, queue, is_remote, no_daily, &today_str, now);

    reconcile_res?;
    if is_remote || no_daily || processing_is_deferred(journal) {
        return Ok(());
    }
    run_catchup_drain(journal, queue, &BTreeSet::from([today_str]), &[], now)
}

pub(crate) fn today_sense_repair_argv(day: &str) -> Vec<String> {
    vec![
        "journal".to_string(),
        "think".to_string(),
        "-v".to_string(),
        "--day".to_string(),
        day.to_string(),
        "--sense-batch".to_string(),
    ]
}

/// Check and submit a sense-only repair task for today's unprocessed observations.
pub(crate) fn run_today_sense_repair(
    journal: &Path,
    queue: &TaskQueue,
    is_remote: bool,
    no_daily: bool,
    today_str: &str,
    now: SystemTime,
) -> Option<(Vec<String>, SubmitOutcome)> {
    if is_remote
        || no_daily
        || processing_is_deferred(journal)
        || no_thinking_engine_chosen(journal)
    {
        return None;
    }
    if !solstone_core_system::catchup::eligible_or_fail_open(journal, today_str, false, now) {
        return None;
    }
    let today_dir = journal.join("chronicle").join(today_str);
    if !today_dir.exists() {
        return None;
    }
    let work = match solstone_core_sense::batch::scan_unprocessed(
        journal, &today_dir, None, None, None,
    ) {
        Ok(work) => work,
        Err(e) => {
            log::warn!(
                "failed to scan unprocessed observations for today's sense repair ({today_str}): {e}"
            );
            return None;
        }
    };
    if work.is_empty() {
        return None;
    }
    let argv = today_sense_repair_argv(today_str);
    let reference = format!("supervisor-sense-{today_str}");
    if queue.contains_reference(&reference) {
        log::debug!("today's sense repair task already referenced in queue: {reference}");
        return Some((argv, SubmitOutcome::DuplicateQueuedReference));
    }
    let outcome = submit_think(queue, argv.clone(), today_str, reference);
    match outcome {
        SubmitOutcome::Rejected => {
            log::warn!("today's sense repair task rejected");
        }
        ref other => {
            log::debug!("today's sense repair submit outcome: {other:?}");
        }
    }
    Some((argv, outcome))
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

fn run_activity_retry_drain(journal: &Path, queue: &TaskQueue, now_ms: i64) -> Result<(), String> {
    for retry in solstone_core_think_cli::due_activity_retries(journal, now_ms)?
        .into_iter()
        .take(8)
    {
        let reference = format!(
            "supervisor-activity-{}",
            serde_json::to_string(&retry).map_err(|e| e.to_string())?
        );
        if queue.contains_reference(&reference) {
            continue;
        }
        let argv = activity_retry_argv(&retry);
        let _ = submit_think(queue, argv, &retry.day, reference);
    }
    Ok(())
}

fn activity_retry_argv(retry: &solstone_core_think_cli::ActivityRetry) -> Vec<String> {
    vec![
        "journal".to_owned(),
        "think".to_owned(),
        "--day".to_owned(),
        retry.day.clone(),
        "--facet".to_owned(),
        retry.facet.clone(),
        "--activity".to_owned(),
        retry.activity.clone(),
    ]
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
    let cmd = TaskArgv::from_wire(argv).expect("supervisor constructs a non-empty argv");
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
            vec![
                "journal".to_owned(),
                "brain".to_owned(),
                "refresh".to_owned(),
                "--expected-fingerprint".to_owned(),
                expected_fingerprint_sha256.clone(),
            ],
            format!("brain-refresh:local-ready:{expected_fingerprint_sha256}"),
        ),
    }
}

/// A Windows Parakeet launch rotates its in-memory loopback credential. Sense
/// is the sole managed service that can spawn native transcription children,
/// so replace that one process tree before reconciling a ready provider result.
/// The port/runtime record never becomes readable as a credential source.
#[cfg(windows)]
fn synchronize_parakeet_sense_credentials(state: &mut SupervisorState) -> bool {
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
    handle_activity_recorded(state, &message);
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
            log::warn!("supervisor: {error}");
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
        log::warn!("supervisor: request rejected");
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
        log::warn!("supervisor: catchup drain request failed: {error}");
    }
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
        .unwrap_or_else(|| chrono::Local::now().format("%Y%m%d").to_string());
    if message_truthy(message, "batch") {
        log::debug!("supervisor: batch observed segment held for daily catchup: {day}/{segment}");
        return;
    }
    if processing_is_deferred(&state.journal) || no_thinking_engine_chosen(&state.journal) {
        log::debug!(
            "supervisor: observed segment held by processing configuration: {day}/{segment}"
        );
        return;
    }
    let stream = message_string(message, "stream").map(str::to_owned);
    // ⛔ An MCP audit record is not an observation of the owner's world. The
    // audit writer publishes into `chronicle/<day>/mcp.agent/<segment>/` and
    // notifies Callosum like any other new segment, and the generic handler
    // below would answer by submitting `journal think` for it — one process
    // per agent tool call, writing an idle sense artifact into the very stream
    // the owner-agent boundary keeps out of enrichment. Nothing is read and no
    // model is called, so this is cost and contamination rather than a leak,
    // but the plan's "audit records are excluded from sense/talent enrichment"
    // has to hold for the process as well as the content.
    //
    if is_mcp_audit_segment(stream.as_deref()) {
        log::debug!("supervisor: MCP audit segment is not enriched: {day}/{segment}");
        return;
    }
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

/// Whether an observed segment is an MCP audit record.
///
/// ⚠ The stream name is duplicated from `solstone-core-mcp-audit`'s
/// `AUDIT_STREAM` rather than imported: this guard must hold whether or not the
/// endpoint feature is compiled into this build, because the records can
/// predate the binary that finds them.
fn is_mcp_audit_segment(stream: Option<&str>) -> bool {
    stream == Some("mcp.agent")
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
        log::warn!("supervisor: activity.recorded message missing id, facet, or day");
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
        vec!["journal".to_owned(), "heartbeat".to_owned()],
        "supervisor-heartbeat".to_owned(),
        None,
        None,
    );
}

fn handle_segment_event_log(journal: &Path, message: &CallosumEnvelope) {
    if !matches!(message.tract.as_str(), "observe" | "think" | "activity") {
        return;
    }
    let (Some(day), Some(segment)) = (
        message_string(message, "day"),
        message_string(message, "segment"),
    ) else {
        return;
    };
    let day_dir = match day_path(journal, Some(day), false) {
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

    use super::*;

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
            journal: journal.to_path_buf(),
            is_remote_mode: false,
            no_daily: true,
            server,
            connection,
            queue,
            last_sync_snapshot: None,
            stale_heartbeats: Vec::new(),
            shutdown_started: std::sync::atomic::AtomicBool::new(false),
            started: Instant::now(),
            scheduler: None,
            recorded_schedule_completions: BTreeSet::new(),
            app_processes: Vec::new(),
            local,
            parakeet,
            flush: FlushState::default(),
            daily: DailyState { last_day: None },
            last_retry_expiry_drain: Instant::now(),
            last_activity_retry_drain: Instant::now(),
            activity_retry_seed_day: None,
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

    #[test]
    fn activity_retry_drain_queues_old_source_identity_once_independent_of_daily_markers() {
        use sha2::{Digest, Sha256};
        let bed = Bed::new("activity-retry");
        let queue = queue(&bed.root);
        let identity = solstone_core_think_cli::ActivityRetry {
            day: "20200101".to_owned(),
            facet: "personal".to_owned(),
            activity: "reading_090000".to_owned(),
        };
        let directory = bed.root.join("health/activity-work");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!(
            "{:x}.json",
            Sha256::digest(serde_json::to_vec(&identity).unwrap())
        ));
        fs::write(&path, serde_json::to_vec(&json!({"version":1,"identity":identity,"input_hash":"fixture","remaining":["participation"],"uses":{},"attempts":1,"next_attempt_ms":1000})).unwrap()).unwrap();
        run_activity_retry_drain(&bed.root, &queue, 999).unwrap();
        assert_eq!(pending(&queue), 0);
        run_activity_retry_drain(&bed.root, &queue, 1000).unwrap();
        assert_eq!(pending(&queue), 1);
        run_activity_retry_drain(&bed.root, &queue, 1001).unwrap();
        assert_eq!(pending(&queue), 1);
        assert_eq!(
            activity_retry_argv(&identity),
            vec![
                "journal",
                "think",
                "--day",
                "20200101",
                "--facet",
                "personal",
                "--activity",
                "reading_090000"
            ]
        );
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, day).expect("fixture date")
    }

    fn wall_time(day: u32, seconds: u64) -> SystemTime {
        let midnight = date(day).and_hms_opt(0, 0, 0).unwrap().and_utc();
        SystemTime::from(midnight) + Duration::from_secs(seconds)
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
                        "next_retry_at": wall_time(3, 10).duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
                    },
                    "20260103:segment-repair": {
                        "day": "20260103",
                        "command_kind": "segment-repair",
                        "active": null,
                        "next_retry_at": wall_time(3, 10).duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
                    },
                },
            }))
            .expect("retry state"),
        )
        .expect("write retry state");

        let queue = queue(&bed.root);
        let origin = Instant::now();
        let mut last_drain = origin;
        let now = wall_time(3, 10);

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
    fn scan_finished_moves_a_past_tick_forward_and_keeps_a_future_one() {
        let past = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let before = Instant::now();
        assert!(
            scan_finished(past) >= before,
            "a scan that took time ends later"
        );
        let future = Instant::now() + Duration::from_secs(30);
        assert_eq!(scan_finished(future), future, "a synthetic tick is kept");
    }

    #[test]
    fn retry_expiry_throttle_restarts_when_the_scan_finishes_not_when_it_starts() {
        let bed = Bed::new("retry-expiry-finish");
        bed.enable_thinking();
        bed.updated_day("20260101");
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
                        "next_retry_at": wall_time(3, 10).duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
                    },
                },
            }))
            .expect("retry state"),
        )
        .expect("write retry state");
        let queue = queue(&bed.root);
        // The loop-top instant is already in the past when the scan starts.
        let tick = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let mut last_drain = tick
            .checked_sub(RETRY_EXPIRY_INTERVAL)
            .expect("host uptime exceeds the interval");
        let before = Instant::now();
        handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(3),
            tick,
            wall_time(3, 10),
        )
        .expect("drain");
        let after = Instant::now();
        assert_eq!(pending(&queue), 1, "the drain ran");
        assert!(
            before <= last_drain && last_drain <= after,
            "the marker is stamped by the handler, from the scan and not from the tick or a later time"
        );
    }

    #[test]
    fn retry_expiry_throttle_restarts_when_the_scan_fails_too() {
        let bed = Bed::new("retry-expiry-failure");
        bed.enable_thinking();
        // With a thinking engine chosen the scan creates its health directory first; a file
        // where the directory belongs makes it fail after the drain has begun.
        fs::write(bed.root.join("health"), b"not a directory").expect("block health directory");
        let queue = queue(&bed.root);
        let tick = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let mut last_drain = tick
            .checked_sub(RETRY_EXPIRY_INTERVAL)
            .expect("host uptime exceeds the interval");
        let before = Instant::now();
        let outcome = handle_retry_expiry_drain(
            false,
            false,
            &bed.root,
            &queue,
            &mut last_drain,
            date(3),
            tick,
            wall_time(3, 10),
        );
        let after = Instant::now();
        assert!(outcome.is_err(), "the fixture must make the scan fail");
        assert!(
            before <= last_drain && last_drain <= after,
            "a failed scan is stamped too, from the scan and not from the tick or a later time"
        );
    }

    #[test]
    fn a_scan_that_takes_time_starts_the_throttle_from_its_end_whether_it_succeeds_or_fails() {
        for succeeds in [true, false] {
            let tick = Instant::now()
                .checked_sub(Duration::from_secs(5))
                .expect("host uptime exceeds five seconds");
            let mut last_drain = tick
                .checked_sub(RETRY_EXPIRY_INTERVAL)
                .expect("host uptime exceeds the interval");
            let mut scan_ended = None;
            let mut scans = 0;
            let outcome =
                retry_expiry_drain_with(false, false, &mut last_drain, date(3), tick, |_| {
                    scans += 1;
                    std::thread::sleep(Duration::from_millis(20));
                    scan_ended = Some(Instant::now());
                    if succeeds {
                        Ok(())
                    } else {
                        Err(CatchupError::State("scripted".to_owned()))
                    }
                });
            let after = Instant::now();
            assert_eq!(scans, 1);
            assert_eq!(outcome.is_ok(), succeeds);
            let scan_ended = scan_ended.expect("the scan ran");
            assert!(
                scan_ended <= last_drain && last_drain <= after,
                "the marker is the scan's end (succeeds: {succeeds}), not its start, the tick or a later time"
            );
            // A tick just short of an interval after the scan's END is throttled. Measured from
            // a stamp taken when the scan started it would already scan, so this pins the end.
            let next_tick = scan_ended + RETRY_EXPIRY_INTERVAL - Duration::from_millis(5);
            let mut again = 0;
            retry_expiry_drain_with(false, false, &mut last_drain, date(3), next_tick, |_| {
                again += 1;
                Ok(())
            })
            .expect("throttled");
            assert_eq!(again, 0, "throttled until an interval after the scan's end");
        }
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
                    "next_retry_at": wall_time(7, 10).duration_since(UNIX_EPOCH).unwrap().as_secs_f64(),
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
            wall_time(7, 10),
        )
        .expect("expired retry tick");

        assert_eq!(pending(&queue), 4);
    }

    #[test]
    fn retry_expiry_does_not_force_a_day_whose_marker_pair_is_already_clean() {
        let bed = Bed::new("retry-expiry-clean");
        bed.enable_thinking();
        // This fixture tests raw-marker retry behavior with no enabled daily work.
        let (talent, apps) = solstone_core_system::daily_coverage::package_roots().unwrap();
        let overrides =
            solstone_core_system::daily_coverage::daily_configs(&bed.root, &talent, &apps)
                .unwrap()
                .into_iter()
                .map(|config| {
                    let key = match config.key.split_once(':') {
                        Some((app, name)) => format!("talent.{app}.{name}"),
                        None => format!("talent.system.{}", config.key),
                    };
                    (key, json!({"disabled": true}))
                })
                .collect::<Map<String, Value>>();
        fs::write(
            bed.root.join("config/journal.json"),
            serde_json::to_vec(&json!({
                "identity":{"timezone":"UTC"}, "providers":{"active":{"provider":"local"}},
                "talent_overrides": overrides
            }))
            .unwrap(),
        )
        .unwrap();
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
            wall_time(2, 10),
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
    fn activity_retry_drain_all_outcomes_and_timing() {
        for seed_succeeds in [true, false] {
            for drain_succeeds in [true, false] {
                let tick = Instant::now()
                    .checked_sub(Duration::from_secs(5))
                    .expect("host uptime exceeds five seconds");
                let mut last_drain = tick
                    .checked_sub(RETRY_EXPIRY_INTERVAL)
                    .expect("host uptime exceeds the interval");
                let mut seed_day = None;
                let mut seed_calls = 0;
                let mut drain_calls = 0;
                let mut drain_ended = None;

                let (seed_outcome, drain_outcome) = activity_retry_drain_with(
                    false,
                    false,
                    false,
                    false,
                    &mut seed_day,
                    &mut last_drain,
                    "20260102",
                    tick,
                    || {
                        seed_calls += 1;
                        if seed_succeeds {
                            Ok(())
                        } else {
                            Err("seed-failed".to_owned())
                        }
                    },
                    || {
                        drain_calls += 1;
                        std::thread::sleep(Duration::from_millis(20));
                        drain_ended = Some(Instant::now());
                        if drain_succeeds {
                            Ok(())
                        } else {
                            Err("drain-failed".to_owned())
                        }
                    },
                );
                let after = Instant::now();

                assert_eq!(seed_calls, 1);
                assert_eq!(drain_calls, 1);
                assert_eq!(
                    seed_outcome.as_ref().map(Result::is_ok),
                    Some(seed_succeeds)
                );
                assert_eq!(
                    drain_outcome.as_ref().map(Result::is_ok),
                    Some(drain_succeeds)
                );

                if seed_succeeds {
                    assert_eq!(seed_day.as_deref(), Some("20260102"));
                } else {
                    assert_eq!(seed_day, None);
                }

                let drain_ended = drain_ended.expect("drain ran");
                assert!(
                    drain_ended <= last_drain && last_drain <= after,
                    "D <= M <= A bound holds (seed_ok: {seed_succeeds}, drain_ok: {drain_succeeds})"
                );

                let marker = last_drain;

                // Tick just before M + INTERVAL is throttled (0 calls, marker unchanged)
                let before_interval_tick =
                    marker + RETRY_EXPIRY_INTERVAL - Duration::from_millis(5);
                let mut seed_again = 0;
                let mut drain_again = 0;
                let (s2, d2) = activity_retry_drain_with(
                    false,
                    false,
                    false,
                    false,
                    &mut seed_day,
                    &mut last_drain,
                    "20260102",
                    before_interval_tick,
                    || {
                        seed_again += 1;
                        Ok(())
                    },
                    || {
                        drain_again += 1;
                        Ok(())
                    },
                );
                assert_eq!(seed_again, 0);
                assert_eq!(drain_again, 0);
                assert_eq!(s2, None);
                assert_eq!(d2, None);
                assert_eq!(last_drain, marker, "marker must not change when throttled");

                // Tick at M + INTERVAL runs
                let at_interval_tick = marker + RETRY_EXPIRY_INTERVAL;
                let (s3, d3) = activity_retry_drain_with(
                    false,
                    false,
                    false,
                    false,
                    &mut seed_day,
                    &mut last_drain,
                    "20260102",
                    at_interval_tick,
                    || {
                        seed_again += 1;
                        Ok(())
                    },
                    || {
                        drain_again += 1;
                        Ok(())
                    },
                );
                assert_eq!(drain_again, 1);
                assert!(d3.is_some());
                if seed_succeeds {
                    // Already marked, seed skipped
                    assert_eq!(seed_again, 0);
                    assert_eq!(s3, None);
                } else {
                    // Was not marked on previous failure, so retries seed
                    assert_eq!(seed_again, 1);
                    assert!(s3.is_some());
                }
            }
        }
    }

    #[test]
    fn activity_retry_drain_skip_predicates_and_already_seeded() {
        let tick = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let initial_drain = tick
            .checked_sub(RETRY_EXPIRY_INTERVAL)
            .expect("host uptime exceeds the interval");

        // 1. Already seeded today -> seed 0, drain 1, marker follows drain (D <= M <= A)
        let mut seed_day = Some("20260102".to_owned());
        let mut last_drain = initial_drain;
        let mut seed_calls = 0;
        let mut drain_calls = 0;
        let mut drain_ended = None;
        let (s, d) = activity_retry_drain_with(
            false,
            false,
            false,
            false,
            &mut seed_day,
            &mut last_drain,
            "20260102",
            tick,
            || {
                seed_calls += 1;
                Ok(())
            },
            || {
                drain_calls += 1;
                std::thread::sleep(Duration::from_millis(20));
                drain_ended = Some(Instant::now());
                Ok(())
            },
        );
        let after = Instant::now();
        assert_eq!(seed_calls, 0);
        assert_eq!(drain_calls, 1);
        assert_eq!(s, None);
        assert_eq!(d, Some(Ok(())));
        assert_eq!(seed_day.as_deref(), Some("20260102"));
        let drain_ended = drain_ended.expect("drain ran");
        assert!(drain_ended <= last_drain && last_drain <= after);

        // 2. Skip twins: no_daily, remote, deferred, no_engine, not-yet-due
        let skip_cases = [
            (true, false, false, false, initial_drain), // no_daily
            (false, true, false, false, initial_drain), // remote
            (false, false, true, false, initial_drain), // deferred
            (false, false, false, true, initial_drain), // no_engine
            (false, false, false, false, tick),         // not-yet-due (last_drain = tick)
        ];

        for (no_daily, remote, deferred, no_engine, drain_ts) in skip_cases {
            let mut s_day = None;
            let mut l_drain = drain_ts;
            let mut s_count = 0;
            let mut d_count = 0;
            let (s_res, d_res) = activity_retry_drain_with(
                no_daily,
                remote,
                deferred,
                no_engine,
                &mut s_day,
                &mut l_drain,
                "20260102",
                tick,
                || {
                    s_count += 1;
                    Ok(())
                },
                || {
                    d_count += 1;
                    Ok(())
                },
            );
            assert_eq!(s_count, 0);
            assert_eq!(d_count, 0);
            assert_eq!(s_res, None);
            assert_eq!(d_res, None);
            assert_eq!(s_day, None);
            assert_eq!(l_drain, drain_ts);
        }
    }

    #[test]
    fn daily_catchup_failure_suppresses_same_tick_retry_expiry() {
        let tick = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let initial_drain = tick
            .checked_sub(RETRY_EXPIRY_INTERVAL)
            .expect("host uptime exceeds the interval");
        let mut last_drain = initial_drain;

        let mut rollover_calls = 0;
        let mut retry_calls = 0;
        let mut catchup_ended = None;

        let outcome = compose_daily_and_retry_expiry_with(
            &mut last_drain,
            tick,
            || {
                rollover_calls += 1;
                std::thread::sleep(Duration::from_millis(20));
                catchup_ended = Some(Instant::now());
                Err(CatchupError::State(
                    "distinctive-daily-catchup-failure".to_owned(),
                ))
            },
            |_last_drain| {
                retry_calls += 1;
                Ok(())
            },
        );
        let after = Instant::now();

        assert_eq!(rollover_calls, 1);
        assert_eq!(
            retry_calls, 0,
            "retry-expiry must not run in the same tick as a failed rollover"
        );
        assert!(matches!(
            outcome,
            Err(DailyOrExpiryError::Daily(CatchupError::State(ref msg)))
                if msg == "distinctive-daily-catchup-failure"
        ));
        let catchup_ended = catchup_ended.expect("catchup ran");
        assert!(
            catchup_ended <= last_drain && last_drain <= after,
            "marker must be stamped from completion via scan_finished(tick)"
        );
    }

    #[test]
    fn daily_and_retry_expiry_not_due_and_success_composition() {
        let tick = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let initial_drain = tick
            .checked_sub(RETRY_EXPIRY_INTERVAL)
            .expect("host uptime exceeds the interval");

        // 1. Rollover NotDue (Ok(false)) -> retry-expiry called, compose does not stamp
        let mut last_drain = initial_drain;
        let mut rollover_calls = 0;
        let mut retry_calls = 0;
        let outcome = compose_daily_and_retry_expiry_with(
            &mut last_drain,
            tick,
            || {
                rollover_calls += 1;
                Ok(false)
            },
            |_last_drain| {
                retry_calls += 1;
                Ok(())
            },
        );
        assert_eq!(rollover_calls, 1);
        assert_eq!(retry_calls, 1);
        assert!(outcome.is_ok());
        assert_eq!(
            last_drain, initial_drain,
            "compose does not stamp on NotDue"
        );

        // 2. Rollover Success (Ok(true)) -> retry-expiry 0, stamps marker, returns Ok(())
        let mut last_drain = initial_drain;
        let mut rollover_calls = 0;
        let mut retry_calls = 0;
        let mut catchup_ended = None;
        let outcome = compose_daily_and_retry_expiry_with(
            &mut last_drain,
            tick,
            || {
                rollover_calls += 1;
                std::thread::sleep(Duration::from_millis(20));
                catchup_ended = Some(Instant::now());
                Ok(true)
            },
            |_last_drain| {
                retry_calls += 1;
                Ok(())
            },
        );
        let after = Instant::now();
        assert_eq!(rollover_calls, 1);
        assert_eq!(
            retry_calls, 0,
            "retry-expiry must not run when rollover drained"
        );
        assert!(outcome.is_ok());
        let catchup_ended = catchup_ended.expect("catchup ran");
        assert!(catchup_ended <= last_drain && last_drain <= after);
    }

    #[test]
    fn daily_catchup_failure_allows_retry_expiry_after_interval() {
        let tick = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .expect("host uptime exceeds five seconds");
        let initial_drain = tick
            .checked_sub(RETRY_EXPIRY_INTERVAL)
            .expect("host uptime exceeds the interval");
        let mut last_drain = initial_drain;

        // Tick 0: Failed rollover stamps last_drain (M), retry-expiry not called
        let outcome0 = compose_daily_and_retry_expiry_with(
            &mut last_drain,
            tick,
            || Err(CatchupError::State("rollover-error".to_owned())),
            |_drain| panic!("retry-expiry must not be called on failed rollover"),
        );
        assert!(matches!(outcome0, Err(DailyOrExpiryError::Daily(_))));
        let marker = last_drain;

        // Tick 1: Just before M + INTERVAL, rollover is NotDue (Ok(false)).
        // retry_expiry_drain_with is called via compose, but throttled (scan called 0 times, marker unchanged).
        let tick_before = marker + RETRY_EXPIRY_INTERVAL - Duration::from_millis(5);
        let mut scan_calls = 0;
        let outcome1 = compose_daily_and_retry_expiry_with(
            &mut last_drain,
            tick_before,
            || Ok(false),
            |drain| {
                retry_expiry_drain_with(false, false, drain, date(3), tick_before, |_| {
                    scan_calls += 1;
                    Ok(())
                })
            },
        );
        assert!(outcome1.is_ok());
        assert_eq!(
            scan_calls, 0,
            "retry-expiry must be throttled before interval"
        );
        assert_eq!(last_drain, marker, "marker must not change when throttled");

        // Tick 2: At M + INTERVAL, rollover is NotDue (Ok(false)).
        // retry_expiry_drain_with is called via compose, runs scan (1 call) and advances marker.
        let tick_at = marker + RETRY_EXPIRY_INTERVAL;
        let outcome2 = compose_daily_and_retry_expiry_with(
            &mut last_drain,
            tick_at,
            || Ok(false),
            |drain| {
                retry_expiry_drain_with(false, false, drain, date(3), tick_at, |_| {
                    scan_calls += 1;
                    Ok(())
                })
            },
        );
        assert!(outcome2.is_ok());
        assert_eq!(scan_calls, 1, "retry-expiry must run at/after interval");
        assert!(
            marker < last_drain,
            "marker advances after retry-expiry scan"
        );
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
                        "active": {"ref": "lost", "started_at": wall_time(3, 1).duration_since(UNIX_EPOCH).unwrap().as_secs_f64()},
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

        initialize_catchup(&bed.root, &queue, false, false, date(3), wall_time(3, 20))
            .expect("startup catchup");

        assert_eq!(pending(&queue), 1, "only fresh past-day dirtiness drains");
        let state: Value = serde_json::from_slice(
            &fs::read(bed.root.join("health/catchup-state.json")).expect("catchup state"),
        )
        .expect("catchup JSON");
        let stale = &state["entries"]["20260101:daily-catchup"];
        assert_eq!(stale["last_outcome"], "interrupted");
        assert_eq!(
            stale["next_retry_at"],
            wall_time(3, 620)
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
        );
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
            wall_time(7, 0),
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

    #[test]
    fn today_marker_dirty_all_sensed_skips_repair_submit() {
        let bed = Bed::new("today-all-sensed");
        bed.enable_thinking();
        bed.updated_day("20260102");
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");
        fs::write(seg_dir.join("audio.jsonl"), b"{\"timestamp_ms\": 0}\n").expect("write output");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_repair_gated_by_remote_mode() {
        let bed = Bed::new("today-remote");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            true, // is_remote = true
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_repair_gated_by_deferred_processing() {
        let bed = Bed::new("today-deferred");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");
        fs::write(
            bed.root.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}},"processing":{"mode":"deferred"}}"#,
        )
        .expect("deferred config");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_repair_gated_by_no_daily() {
        let bed = Bed::new("today-no-daily");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            true, // no_daily = true
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_repair_gated_by_no_thinking_engine() {
        let bed = Bed::new("today-no-engine");
        // No bed.enable_thinking()
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_repair_gated_by_eligibility_backoff() {
        let bed = Bed::new("today-backoff");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let fingerprint =
            solstone_core_system::catchup::read_raw_input_fingerprint(&bed.root, "20260102")
                .expect("fingerprint");

        fs::create_dir_all(bed.root.join("health")).expect("catchup health");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    "20260102:daily-catchup": {
                        "day": "20260102",
                        "command_kind": "daily-catchup",
                        "active": null,
                        "fingerprint": fingerprint,
                        "next_retry_at": 100.0,
                    }
                }
            }))
            .expect("retry state"),
        )
        .expect("write retry state");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_repair_fails_open_on_unreadable_eligibility() {
        let bed = Bed::new("today-fail-open");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        fs::create_dir_all(bed.root.join("health")).expect("catchup health");
        fs::write(
            bed.root.join("health/catchup-state.json"),
            b"not-valid-json",
        )
        .expect("write corrupt state");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(pending(&queue), 1);
    }

    #[test]
    fn today_repair_runs_when_catchup_capability_unavailable() {
        let bed = Bed::new("today-capability-unavailable");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        let result = initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Err(CatchupError::CapabilityUnavailable),
        );

        assert!(matches!(result, Err(CatchupError::CapabilityUnavailable)));
        assert_eq!(
            pending(&queue),
            1,
            "today's repair must run even when capability is unavailable"
        );
    }

    #[test]
    fn today_orphan_submits_one_sense_batch_task() {
        let bed = Bed::new("today-orphan-submit");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        let outcome = run_today_sense_repair(
            &bed.root,
            &queue,
            false,
            false,
            "20260102",
            UNIX_EPOCH + Duration::from_secs(10),
        );

        let (argv, submit_outcome) = outcome.expect("repair outcome");
        assert_eq!(
            argv,
            vec![
                "journal".to_string(),
                "think".to_string(),
                "-v".to_string(),
                "--day".to_string(),
                "20260102".to_string(),
                "--sense-batch".to_string(),
            ]
        );
        assert!(matches!(
            submit_outcome,
            SubmitOutcome::Pending | SubmitOutcome::Queued | SubmitOutcome::Dispatched
        ));
        assert_eq!(pending(&queue), 1);
        assert!(queue.contains_reference("supervisor-sense-20260102"));
    }

    #[test]
    fn catchup_state_bytes_unchanged_on_repair_submit_and_fail() {
        let bed = Bed::new("today-state-present");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let state_path = bed.root.join("health/catchup-state.json");
        fs::create_dir_all(state_path.parent().unwrap()).expect("health dir");
        let initial_bytes = b"{\"version\":1,\"entries\":{\"20260101:daily-catchup\":{\"day\":\"20260101\",\"command_kind\":\"daily-catchup\",\"active\":null,\"next_retry_at\":null},\"20260102:daily-catchup\":{\"day\":\"20260102\",\"command_kind\":\"daily-catchup\",\"active\":null,\"next_retry_at\":null}}}";
        fs::write(&state_path, initial_bytes).expect("write state");

        let queue1 = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue1,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup 1");
        assert_eq!(fs::read(&state_path).expect("read state 1"), initial_bytes);

        let queue2 = queue(&bed.root);
        queue2.set_ready();
        queue2.shutdown();
        let _ = run_today_sense_repair(
            &bed.root,
            &queue2,
            false,
            false,
            "20260102",
            UNIX_EPOCH + Duration::from_secs(10),
        );
        assert_eq!(fs::read(&state_path).expect("read state 2"), initial_bytes);

        let bed_absent = Bed::new("today-state-absent");
        bed_absent.enable_thinking();
        let seg_dir_absent = bed_absent.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir_absent).expect("segment dir");
        fs::write(seg_dir_absent.join("audio.m4a"), b"audio-data").expect("write audio");
        let absent_state_path = bed_absent.root.join("health/catchup-state.json");
        assert!(!absent_state_path.exists());

        let queue_absent1 = queue(&bed_absent.root);
        initialize_catchup_with_reconcile(
            &bed_absent.root,
            &queue_absent1,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup absent 1");
        assert!(!absent_state_path.exists());

        let queue_absent2 = queue(&bed_absent.root);
        queue_absent2.set_ready();
        queue_absent2.shutdown();
        let _ = run_today_sense_repair(
            &bed_absent.root,
            &queue_absent2,
            false,
            false,
            "20260102",
            UNIX_EPOCH + Duration::from_secs(10),
        );
        assert!(!absent_state_path.exists());
    }

    #[test]
    fn marker_files_untouched_after_successful_sense_repair() {
        let bed = Bed::new("today-markers-untouched");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let health_dir = bed.root.join("chronicle/20260102/health");
        fs::create_dir_all(&health_dir).expect("health dir");
        let stream_marker = health_dir.join("stream.updated");
        let daily_marker = health_dir.join("daily.updated");

        let stream_bytes = b"100.0\n";
        let daily_bytes = b"50.0\n";
        fs::write(&stream_marker, stream_bytes).expect("write stream marker");
        fs::write(&daily_marker, daily_bytes).expect("write daily marker");

        let queue = queue(&bed.root);
        initialize_catchup_with_reconcile(
            &bed.root,
            &queue,
            false,
            false,
            date(2),
            UNIX_EPOCH + Duration::from_secs(10),
            |_, _| Ok(()),
        )
        .expect("initialize catchup");

        assert_eq!(
            fs::read(&stream_marker).expect("stream marker"),
            stream_bytes
        );
        assert_eq!(fs::read(&daily_marker).expect("daily marker"), daily_bytes);
    }

    #[test]
    fn ordinary_catchup_drain_still_selects_day_after_three_failed_today_repairs() {
        let bed = Bed::new("catchup-drain-after-failed-repair");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let health_dir = bed.root.join("chronicle/20260102/health");
        fs::create_dir_all(&health_dir).expect("health dir");
        fs::write(health_dir.join("stream.updated"), b"100.0\n").expect("write stream marker");

        let now = wall_time(3, 10);
        for _ in 0..3 {
            let q = queue(&bed.root);
            q.set_ready();
            q.shutdown();
            let _ = run_today_sense_repair(&bed.root, &q, false, false, "20260102", now);
        }

        let drain_queue = queue(&bed.root);
        run_catchup_drain(
            &bed.root,
            &drain_queue,
            &BTreeSet::from(["20260103".to_string()]),
            &[],
            now,
        )
        .expect("catchup drain");

        assert_eq!(pending(&drain_queue), 1);
        assert!(drain_queue.contains_reference("supervisor-catchup-20260102"));
    }

    #[test]
    fn ordinary_catchup_drain_still_selects_day_after_successful_today_repair() {
        let bed = Bed::new("catchup-drain-after-successful-repair");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let health_dir = bed.root.join("chronicle/20260102/health");
        fs::create_dir_all(&health_dir).expect("health dir");
        fs::write(health_dir.join("stream.updated"), b"100.0\n").expect("write stream marker");

        let now = wall_time(3, 10);
        let q = queue(&bed.root);
        let outcome = run_today_sense_repair(&bed.root, &q, false, false, "20260102", now);
        assert!(outcome.is_some());

        let drain_queue = queue(&bed.root);
        run_catchup_drain(
            &bed.root,
            &drain_queue,
            &BTreeSet::from(["20260103".to_string()]),
            &[],
            now,
        )
        .expect("catchup drain");

        assert_eq!(pending(&drain_queue), 1);
        assert!(drain_queue.contains_reference("supervisor-catchup-20260102"));
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
    fn today_repair_skips_when_all_observations_already_sensed() {
        let bed = Bed::new("today-all-sensed");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("screen.webm"), b"video").expect("write video");
        fs::write(
            seg_dir.join("screen.jsonl"),
            "{\"frame_id\":1,\"timestamp\":0.0}\n",
        )
        .expect("evidence sidecar");

        let queue = queue(&bed.root);
        let outcome = run_today_sense_repair(
            &bed.root,
            &queue,
            false,
            false,
            "20260102",
            UNIX_EPOCH + Duration::from_secs(10),
        );
        assert!(outcome.is_none());
        assert_eq!(pending(&queue), 0);
    }

    #[test]
    fn today_nothing_outstanding_submits_nothing_and_writes_no_attempt() {
        let bed = Bed::new("today-nothing-outstanding");
        bed.enable_thinking();
        fs::create_dir_all(bed.root.join("chronicle/20260102")).expect("day dir");

        let queue = queue(&bed.root);
        let outcome = run_today_sense_repair(
            &bed.root,
            &queue,
            false,
            false,
            "20260102",
            UNIX_EPOCH + Duration::from_secs(10),
        );
        assert!(outcome.is_none());
        assert_eq!(pending(&queue), 0);
        assert!(!bed.root.join("health/catchup-state.json").exists());
        assert!(
            !bed.root
                .join("chronicle/20260102/health/sense-day.lease")
                .exists()
        );
    }

    #[test]
    fn today_repair_coalesces_when_reference_already_queued() {
        let bed = Bed::new("today-coalesce");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        let now = UNIX_EPOCH + Duration::from_secs(10);
        let first = run_today_sense_repair(&bed.root, &queue, false, false, "20260102", now);
        assert!(first.is_some());
        assert_eq!(pending(&queue), 1);

        let second = run_today_sense_repair(&bed.root, &queue, false, false, "20260102", now);
        let (_, second_outcome) = second.expect("second repair result");
        assert_eq!(second_outcome, SubmitOutcome::DuplicateQueuedReference);
        assert_eq!(pending(&queue), 1);
    }

    #[test]
    fn today_repair_rejected_submission_is_logged_not_discarded() {
        let bed = Bed::new("today-rejected");
        bed.enable_thinking();
        let seg_dir = bed.root.join("chronicle/20260102/120000_1");
        fs::create_dir_all(&seg_dir).expect("segment dir");
        fs::write(seg_dir.join("audio.m4a"), b"audio-data").expect("write audio");

        let queue = queue(&bed.root);
        queue.set_ready();
        queue.shutdown();

        let outcome = run_today_sense_repair(
            &bed.root,
            &queue,
            false,
            false,
            "20260102",
            UNIX_EPOCH + Duration::from_secs(10),
        );
        let (_, submit_outcome) = outcome.expect("repair outcome");
        assert_eq!(submit_outcome, SubmitOutcome::Rejected);
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
