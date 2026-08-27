// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumConnectionPhase, CallosumEnvelope, CallosumReceiveEvent};

use crate::{
    ProcessObserver, ProcessSample, TopState, acknowledge_restart, fail_discontinuous_restarts,
};

pub const STATUS_TIMEOUT_SECONDS: f64 = 5.0;

/// All time supplied to reduction. `wall_datetime` is a fixture-compatible
/// object (normally `{ "datetime": "..." }`), never an ambient clock read.
#[derive(Clone, Debug)]
pub struct ReductionSample {
    pub wall_seconds: f64,
    pub monotonic_seconds: f64,
    pub wall_datetime: Value,
}

impl ReductionSample {
    #[must_use]
    pub fn fixture(wall_seconds: f64, datetime: impl Into<String>) -> Self {
        Self {
            wall_seconds,
            monotonic_seconds: wall_seconds,
            wall_datetime: json!({"datetime": datetime.into()}),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReductionEffects {
    pub refresh_brain: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopRoute {
    SupervisorStatus,
    SupervisorLifecycle,
    SupervisorQueue,
    LogsExec,
    LogsLine,
    LogsExit,
    ObserveStatus,
    ObserveObserved,
    ThinkStarted,
    ThinkStatus,
    ThinkCompleted,
}

impl std::fmt::Display for TopRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let route = match self {
            Self::SupervisorStatus => "supervisor/status",
            Self::SupervisorLifecycle => "supervisor/lifecycle",
            Self::SupervisorQueue => "supervisor/queue",
            Self::LogsExec => "logs/exec",
            Self::LogsLine => "logs/line",
            Self::LogsExit => "logs/exit",
            Self::ObserveStatus => "observe/status",
            Self::ObserveObserved => "observe/observed",
            Self::ThinkStarted => "think/started",
            Self::ThinkStatus => "think/status",
            Self::ThinkCompleted => "think/completed",
        };
        formatter.write_str(route)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopMalformedKind {
    MissingField(&'static str),
    WrongType(&'static str),
    InvalidValue(&'static str),
    OutOfRange(&'static str),
    DuplicateIdentity(&'static str),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopMalformed {
    pub route: TopRoute,
    pub kind: TopMalformedKind,
}

impl std::fmt::Display for TopMalformed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {:?}", self.route, self.kind)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReductionDisposition {
    Applied(ReductionEffects),
    Ignored,
    Malformed(TopMalformed),
}

impl ReductionDisposition {
    fn effects(&self) -> ReductionEffects {
        match self {
            Self::Applied(effects) => effects.clone(),
            Self::Ignored | Self::Malformed(_) => ReductionEffects::default(),
        }
    }
}

/// Validate and apply a decoded envelope. A route error is visible evidence,
/// never a control-flow error for the owner loop.
pub fn reduce_envelope(
    state: &mut TopState,
    envelope: &CallosumEnvelope,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> ReductionDisposition {
    match (envelope.tract.as_str(), envelope.event.as_str()) {
        ("supervisor", "status") => validated(
            state,
            validate_supervisor_status(&envelope.extra),
            |state, value| commit_supervisor_status(state, value, sample, observer),
        ),
        ("supervisor", "restarting" | "started" | "stopped") => validated(
            state,
            validate_supervisor_lifecycle(&envelope.extra),
            |state, value| commit_supervisor_lifecycle(state, value, &envelope.event, sample),
        ),
        ("supervisor", "queue") => validated(
            state,
            validate_supervisor_queue(&envelope.extra),
            commit_supervisor_queue,
        ),
        ("logs", "exec") => {
            let result = validate_logs_exec(&envelope.extra, state);
            validated(state, result, |state, value| {
                commit_logs_exec(state, value, sample, observer)
            })
        }
        ("logs", "line") => {
            let result = validate_logs_line(&envelope.extra, state);
            validated(state, result, |state, value| {
                commit_logs_line(state, value, sample, observer)
            })
        }
        ("logs", "exit") => validated(
            state,
            validate_logs_exit(&envelope.extra),
            |state, value| commit_logs_exit(state, value, sample, observer),
        ),
        ("observe", "status") => match validate_observe_status(&envelope.extra) {
            Ok(value) if value.fields.is_empty() => ReductionDisposition::Ignored,
            Ok(value) => ReductionDisposition::Applied(commit_observe_status(state, value, sample)),
            Err(malformed) => record_malformed(state, malformed),
        },
        ("observe", "observed") => validated(
            state,
            validate_observe_observed(&envelope.extra),
            commit_observe_observed,
        ),
        ("think", "started") => validated(
            state,
            validate_think_started(&envelope.extra),
            commit_think_started,
        ),
        ("think", "status") => match validate_think_status(&envelope.extra) {
            Ok(value) if value.fields.is_empty() => ReductionDisposition::Ignored,
            Ok(value) => ReductionDisposition::Applied(commit_think_status(state, value)),
            Err(malformed) => record_malformed(state, malformed),
        },
        ("think", "completed") => validated(
            state,
            validate_think_completed(&envelope.extra),
            commit_think_completed,
        ),
        _ => ReductionDisposition::Ignored,
    }
}

fn validated<T>(
    state: &mut TopState,
    result: Result<T, TopMalformed>,
    commit: impl FnOnce(&mut TopState, T) -> ReductionEffects,
) -> ReductionDisposition {
    match result {
        Ok(value) => ReductionDisposition::Applied(commit(state, value)),
        Err(malformed) => record_malformed(state, malformed),
    }
}

fn record_malformed(state: &mut TopState, malformed: TopMalformed) -> ReductionDisposition {
    state.malformed_events = state.malformed_events.saturating_add(1);
    state.last_malformed = Some(malformed.clone());
    ReductionDisposition::Malformed(malformed)
}

/// Apply continuity changes before reducing a tagged envelope.
pub fn apply_receive_event(
    state: &mut TopState,
    event: &CallosumReceiveEvent,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> ReductionEffects {
    match event {
        CallosumReceiveEvent::Envelope {
            generation,
            epoch,
            envelope,
        } => {
            if *generation != state.continuity.generation
                || *epoch != state.continuity.epoch
                || !matches!(
                    state.continuity.connection,
                    CallosumConnectionPhase::Connected
                )
            {
                state.continuity.rejected_receive_events =
                    state.continuity.rejected_receive_events.saturating_add(1);
                return ReductionEffects::default();
            }
            if envelope.tract == "observe"
                && envelope.event == "status"
                && !observe_status_has_display_fields(&envelope.extra)
            {
                return ReductionEffects::default();
            }
            let disposition = reduce_envelope(state, envelope, sample, observer);
            let effects = disposition.effects();
            if matches!(disposition, ReductionDisposition::Applied(_))
                && envelope.tract == "supervisor"
                && matches!(
                    envelope.event.as_str(),
                    "restarting" | "started" | "stopped"
                )
                && let Some(service) = envelope.extra.get("service").and_then(Value::as_str)
            {
                let _ = acknowledge_restart(
                    state,
                    service,
                    envelope.extra.get("restart_id").and_then(Value::as_str),
                    *generation,
                    *epoch,
                    &envelope.event,
                    sample.monotonic_seconds,
                );
            }
            if !matches!(disposition, ReductionDisposition::Applied(_)) {
                return effects;
            }
            match (envelope.tract.as_str(), envelope.event.as_str()) {
                ("supervisor", "status") => {
                    state.continuity.supervisor = crate::DomainRecovery::Complete;
                }
                ("supervisor", "restarting" | "started" | "stopped" | "queue") => {
                    state.continuity.supervisor.record_evidence();
                }
                ("logs", "exec" | "line" | "exit") => {
                    state.continuity.tasks.record_evidence();
                }
                ("observe", "status" | "observed") => {
                    state.continuity.observe.record_evidence();
                }
                ("think", "started" | "status" | "completed") => {
                    state.continuity.think.record_evidence();
                }
                _ => {}
            }
            effects
        }
        CallosumReceiveEvent::Continuity {
            generation,
            epoch,
            phase,
        } => {
            let generation_changed = *generation != state.continuity.generation;
            state.continuity.generation = *generation;
            state.continuity.epoch = *epoch;
            state.continuity.connection = phase.clone();
            if generation_changed {
                let _ = fail_discontinuous_restarts(state, *generation, sample.monotonic_seconds);
            }
            if matches!(phase, CallosumConnectionPhase::Gapped { .. }) {
                state.continuity.supervisor.incomplete();
                state.continuity.tasks.incomplete();
                state.continuity.observe.incomplete();
                state.continuity.think.incomplete();
                state.services.clear();
                state.crashed.clear();
                state.command_queues.clear();
                state.service_status.clear();
                state.cpu_cache.clear();
                state.memory_cache.clear();
                state.cpu_pids.clear();
                state.process_identities.clear();
                state.running_tasks.clear();
                state.finished_tasks.clear();
                state.task_started_at.clear();
                state.last_log_lines.clear();
                state.last_log_at.clear();
                state.observe_status.clear();
                state.observe_last_ts = 0.0;
                state.displayed_mode = "idle".to_owned();
                state.last_active_ts = 0.0;
                state.think_running = false;
                state.think_status.clear();
                state.think_last_completed.clear();
                for attempt in state.restart_attempts.values_mut() {
                    if matches!(
                        attempt.phase,
                        crate::RestartPhase::Pending
                            | crate::RestartPhase::Restarting
                            | crate::RestartPhase::Stopped
                    ) {
                        attempt.phase =
                            crate::RestartPhase::Failed(crate::RestartFailure::Discontinuity);
                        attempt.phase_at = sample.monotonic_seconds;
                        attempt.terminal_at = Some(sample.monotonic_seconds);
                    }
                }
            }
            ReductionEffects::default()
        }
    }
}

fn observe_status_has_display_fields(extra: &Map<String, Value>) -> bool {
    [
        "mode",
        "stream",
        "screencast",
        "tmux",
        "audio",
        "activity",
        "describe",
        "transcribe",
    ]
    .iter()
    .any(|key| extra.contains_key(*key))
}

/// Refresh live task observations and retain finished-task ghosts for exactly
/// five seconds. Missing and zombie tasks become unknown-completion ghosts;
/// access-denied and unavailable tasks remain visible without being removed.
pub fn cleanup_processes(
    state: &mut TopState,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) {
    let expired: Vec<String> = state
        .finished_task_finished_at
        .iter()
        .filter_map(|(reference, finished_at)| {
            (sample.monotonic_seconds - finished_at > STATUS_TIMEOUT_SECONDS)
                .then_some(reference.clone())
        })
        .collect();
    for reference in expired {
        state.finished_tasks.remove(&reference);
        state.finished_task_finished_at.remove(&reference);
    }
    let tasks: Vec<(String, String, u32)> = state
        .running_tasks
        .iter()
        .filter_map(|(reference, task)| {
            let pid = task.get("pid").and_then(Value::as_u64)? as u32;
            let name = task.get("name").and_then(Value::as_str)?.to_owned();
            Some((reference.clone(), name, pid))
        })
        .collect();
    let mut missing = Vec::new();
    for (reference, name, pid) in tasks {
        let process_sample = observer.sample(pid, sample.monotonic_seconds);
        if matches!(
            process_sample,
            ProcessSample::Missing | ProcessSample::Zombie
        ) {
            missing.push((reference, name, pid));
        } else {
            record_process_sample(state, pid, process_sample);
        }
    }
    for (reference, name, pid) in missing {
        let last_log = state
            .last_log_lines
            .remove(&reference)
            .and_then(|line| line.as_array().and_then(|items| items.get(2).cloned()))
            .unwrap_or_else(|| json!(""));
        state.running_tasks.remove(&reference);
        clear_process_display(state, pid);
        observer.forget(pid);
        state.task_started_at.remove(&reference);
        state.last_log_at.remove(&reference);
        state.finished_tasks.insert(reference.clone(), json!({"name":name,"exit_code":Value::Null,"last_log":last_log,"finished_at":sample.wall_seconds}));
        state
            .finished_task_finished_at
            .insert(reference, sample.monotonic_seconds);
    }
}

struct ValidatedSupervisorStatus {
    services: Vec<ValidatedService>,
    crashed: Vec<ValidatedCrash>,
    queues: BTreeMap<String, u64>,
}

struct ValidatedService {
    name: String,
    reference: String,
    pid: u32,
    uptime_seconds: u64,
}

struct ValidatedCrash {
    name: String,
    restart_attempts: u64,
    phase: Option<String>,
}

struct ValidatedLifecycle {
    service: String,
    restart_id: Option<String>,
}

struct ValidatedQueue {
    command: String,
    queued: u64,
}

struct ValidatedExec {
    reference: String,
    name: String,
    pid: u32,
    cmd: Vec<String>,
}

struct ValidatedLine {
    reference: String,
    line: String,
    stream: String,
    task: Option<(String, u32)>,
}

struct ValidatedExit {
    reference: String,
    exit_code: Option<i32>,
    name: Option<String>,
}

struct ValidatedObserveStatus {
    fields: BTreeMap<String, Value>,
    mode: Option<String>,
}

struct ValidatedObserved {
    day: String,
    segment: String,
    duration: u64,
}

struct ValidatedThinkStatus {
    fields: BTreeMap<String, Value>,
}

struct ValidatedThinkCompleted {
    fields: BTreeMap<String, Value>,
}

struct ValidatedThinkStarted;

fn malformed(route: TopRoute, kind: TopMalformedKind) -> TopMalformed {
    TopMalformed { route, kind }
}

fn required_text(
    extra: &Map<String, Value>,
    key: &'static str,
    route: TopRoute,
) -> Result<String, TopMalformed> {
    match extra.get(key) {
        None => Err(malformed(route, TopMalformedKind::MissingField(key))),
        Some(Value::String(value)) if value.is_empty() => {
            Err(malformed(route, TopMalformedKind::InvalidValue(key)))
        }
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(malformed(route, TopMalformedKind::WrongType(key))),
    }
}

fn optional_text(
    extra: &Map<String, Value>,
    key: &'static str,
    route: TopRoute,
) -> Result<Option<String>, TopMalformed> {
    match extra.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(malformed(route, TopMalformedKind::WrongType(key))),
    }
}

fn required_u64(
    extra: &Map<String, Value>,
    key: &'static str,
    route: TopRoute,
) -> Result<u64, TopMalformed> {
    extra
        .get(key)
        .ok_or_else(|| malformed(route, TopMalformedKind::MissingField(key)))?
        .as_u64()
        .ok_or_else(|| malformed(route, TopMalformedKind::WrongType(key)))
}

fn required_pid(
    extra: &Map<String, Value>,
    key: &'static str,
    route: TopRoute,
) -> Result<u32, TopMalformed> {
    u32::try_from(required_u64(extra, key, route)?)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| malformed(route, TopMalformedKind::OutOfRange(key)))
}

fn required_array<'a>(
    extra: &'a Map<String, Value>,
    key: &'static str,
    route: TopRoute,
) -> Result<&'a Vec<Value>, TopMalformed> {
    match extra.get(key) {
        None => Err(malformed(route, TopMalformedKind::MissingField(key))),
        Some(Value::Array(values)) => Ok(values),
        Some(_) => Err(malformed(route, TopMalformedKind::WrongType(key))),
    }
}

fn required_object<'a>(
    extra: &'a Map<String, Value>,
    key: &'static str,
    route: TopRoute,
) -> Result<&'a Map<String, Value>, TopMalformed> {
    match extra.get(key) {
        None => Err(malformed(route, TopMalformedKind::MissingField(key))),
        Some(Value::Object(values)) => Ok(values),
        Some(_) => Err(malformed(route, TopMalformedKind::WrongType(key))),
    }
}

fn validate_supervisor_status(
    extra: &Map<String, Value>,
) -> Result<ValidatedSupervisorStatus, TopMalformed> {
    let route = TopRoute::SupervisorStatus;
    let mut names = BTreeSet::new();
    let mut references = BTreeSet::new();
    let services = required_array(extra, "services", route)?
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| malformed(route, TopMalformedKind::WrongType("services[]")))?;
            let name = required_text(object, "name", route)?;
            let reference = required_text(object, "ref", route)?;
            if !names.insert(name.clone()) || !references.insert(reference.clone()) {
                return Err(malformed(
                    route,
                    TopMalformedKind::DuplicateIdentity("services"),
                ));
            }
            Ok(ValidatedService {
                name,
                reference,
                pid: required_pid(object, "pid", route)?,
                uptime_seconds: required_u64(object, "uptime_seconds", route)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut crash_names = BTreeSet::new();
    let crashed = required_array(extra, "crashed", route)?
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| malformed(route, TopMalformedKind::WrongType("crashed[]")))?;
            let name = required_text(object, "name", route)?;
            if !crash_names.insert(name.clone()) {
                return Err(malformed(
                    route,
                    TopMalformedKind::DuplicateIdentity("crashed"),
                ));
            }
            Ok(ValidatedCrash {
                name,
                restart_attempts: required_u64(object, "restart_attempts", route)?,
                phase: optional_text(object, "phase", route)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let queues = required_object(extra, "queues", route)?
        .iter()
        .map(|(key, value)| {
            if key.is_empty() {
                return Err(malformed(
                    route,
                    TopMalformedKind::InvalidValue("queues key"),
                ));
            }
            value
                .as_u64()
                .map(|value| (key.clone(), value))
                .ok_or_else(|| malformed(route, TopMalformedKind::WrongType("queues[]")))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    Ok(ValidatedSupervisorStatus {
        services,
        crashed,
        queues,
    })
}

fn validate_supervisor_lifecycle(
    extra: &Map<String, Value>,
) -> Result<ValidatedLifecycle, TopMalformed> {
    let route = TopRoute::SupervisorLifecycle;
    let restart_id = optional_text(extra, "restart_id", route)?;
    if restart_id.as_ref().is_some_and(String::is_empty) {
        return Err(malformed(
            route,
            TopMalformedKind::InvalidValue("restart_id"),
        ));
    }
    Ok(ValidatedLifecycle {
        service: required_text(extra, "service", route)?,
        restart_id,
    })
}

fn validate_supervisor_queue(extra: &Map<String, Value>) -> Result<ValidatedQueue, TopMalformed> {
    let route = TopRoute::SupervisorQueue;
    Ok(ValidatedQueue {
        command: required_text(extra, "command", route)?,
        queued: required_u64(extra, "queued", route)?,
    })
}

fn validate_logs_exec(
    extra: &Map<String, Value>,
    state: &TopState,
) -> Result<ValidatedExec, TopMalformed> {
    let route = TopRoute::LogsExec;
    let reference = required_text(extra, "ref", route)?;
    let name = required_text(extra, "name", route)?;
    let pid = required_pid(extra, "pid", route)?;
    if let Some(existing) = state.running_tasks.get(&reference)
        && (existing.get("name").and_then(Value::as_str) != Some(&name)
            || existing.get("pid").and_then(Value::as_u64) != Some(u64::from(pid)))
    {
        return Err(malformed(route, TopMalformedKind::DuplicateIdentity("ref")));
    }
    let cmd = match extra.get("cmd") {
        None => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| malformed(route, TopMalformedKind::WrongType("cmd[]")))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(malformed(route, TopMalformedKind::WrongType("cmd"))),
    };
    Ok(ValidatedExec {
        reference,
        name,
        pid,
        cmd,
    })
}

fn validate_logs_line(
    extra: &Map<String, Value>,
    state: &TopState,
) -> Result<ValidatedLine, TopMalformed> {
    let route = TopRoute::LogsLine;
    let reference = required_text(extra, "ref", route)?;
    let line = match extra.get("line") {
        Some(Value::String(value)) => value.clone(),
        None => return Err(malformed(route, TopMalformedKind::MissingField("line"))),
        Some(_) => return Err(malformed(route, TopMalformedKind::WrongType("line"))),
    };
    let stream = match extra.get("stream") {
        Some(Value::String(value)) if value == "stdout" || value == "stderr" => value.clone(),
        None => return Err(malformed(route, TopMalformedKind::MissingField("stream"))),
        Some(Value::String(_)) => {
            return Err(malformed(route, TopMalformedKind::InvalidValue("stream")));
        }
        Some(_) => return Err(malformed(route, TopMalformedKind::WrongType("stream"))),
    };
    let task = (!state.running_tasks.contains_key(&reference))
        .then(|| {
            Ok((
                required_text(extra, "name", route)?,
                required_pid(extra, "pid", route)?,
            ))
        })
        .transpose()?;
    Ok(ValidatedLine {
        reference,
        line,
        stream,
        task,
    })
}

fn validate_logs_exit(extra: &Map<String, Value>) -> Result<ValidatedExit, TopMalformed> {
    let route = TopRoute::LogsExit;
    let exit_code = match extra.get("exit_code") {
        Some(Value::Null) => None,
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| malformed(route, TopMalformedKind::OutOfRange("exit_code")))?,
        None => {
            return Err(malformed(
                route,
                TopMalformedKind::MissingField("exit_code"),
            ));
        }
    };
    Ok(ValidatedExit {
        reference: required_text(extra, "ref", route)?,
        exit_code,
        name: optional_text(extra, "name", route)?,
    })
}

fn commit_supervisor_status(
    state: &mut TopState,
    status: ValidatedSupervisorStatus,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> ReductionEffects {
    let retained_pids = status
        .services
        .iter()
        .map(|service| service.pid)
        .chain(
            state
                .running_tasks
                .values()
                .filter_map(|task| task.get("pid").and_then(Value::as_u64))
                .filter_map(|pid| u32::try_from(pid).ok()),
        )
        .collect::<BTreeSet<_>>();
    for pid in state.process_identities.keys().copied().collect::<Vec<_>>() {
        if !retained_pids.contains(&pid) {
            observer.forget(pid);
        }
    }
    state.services = status.services.iter().map(|service| json!({"name":service.name,"ref":service.reference,"pid":service.pid,"uptime_seconds":service.uptime_seconds})).collect();
    state.crashed = status
        .crashed
        .iter()
        .map(|crash| match &crash.phase {
            Some(phase) => {
                json!({"name":crash.name,"restart_attempts":crash.restart_attempts,"phase":phase})
            }
            None => json!({"name":crash.name,"restart_attempts":crash.restart_attempts}),
        })
        .collect();
    state.command_queues = status
        .queues
        .into_iter()
        .map(|(key, value)| (key, json!(value)))
        .collect();
    state.selected = state.selected.min(state.services.len().saturating_sub(1));
    state.cpu_pids.clear();
    state.cpu_cache.clear();
    state.memory_cache.clear();
    state.process_identities.clear();
    for service in status.services {
        record_process_sample(
            state,
            service.pid,
            observer.sample(service.pid, sample.monotonic_seconds),
        );
    }
    ReductionEffects::default()
}

fn commit_supervisor_lifecycle(
    state: &mut TopState,
    lifecycle: ValidatedLifecycle,
    event: &str,
    sample: &ReductionSample,
) -> ReductionEffects {
    let _ = lifecycle.restart_id;
    state
        .service_status
        .insert(lifecycle.service, (event.to_owned(), sample.wall_seconds));
    ReductionEffects::default()
}

fn commit_supervisor_queue(state: &mut TopState, queue: ValidatedQueue) -> ReductionEffects {
    if queue.queued == 0 {
        state.command_queues.remove(&queue.command);
    } else {
        state
            .command_queues
            .insert(queue.command, json!(queue.queued));
    }
    ReductionEffects::default()
}

fn commit_logs_exec(
    state: &mut TopState,
    task: ValidatedExec,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> ReductionEffects {
    state.running_tasks.insert(task.reference.clone(), json!({"ref":task.reference,"name":task.name,"pid":task.pid,"cmd":task.cmd,"start_time":sample.wall_datetime}));
    state
        .task_started_at
        .insert(task.reference, sample.monotonic_seconds);
    record_process_sample(
        state,
        task.pid,
        observer.sample(task.pid, sample.monotonic_seconds),
    );
    ReductionEffects::default()
}

fn commit_logs_line(
    state: &mut TopState,
    line: ValidatedLine,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> ReductionEffects {
    if let Some((name, pid)) = line.task {
        state.running_tasks.insert(line.reference.clone(), json!({"ref":line.reference,"name":name,"pid":pid,"cmd":[],"start_time":sample.wall_datetime}));
        state
            .task_started_at
            .insert(line.reference.clone(), sample.monotonic_seconds);
        record_process_sample(state, pid, observer.sample(pid, sample.monotonic_seconds));
    }
    state.last_log_lines.insert(
        line.reference.clone(),
        json!([sample.wall_datetime, line.stream, line.line]),
    );
    state
        .last_log_at
        .insert(line.reference, sample.wall_seconds);
    ReductionEffects::default()
}

fn commit_logs_exit(
    state: &mut TopState,
    exit: ValidatedExit,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> ReductionEffects {
    let task = state.running_tasks.remove(&exit.reference);
    let name = task
        .as_ref()
        .and_then(|task| task.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or(exit.name)
        .unwrap_or_else(|| "unknown".to_owned());
    let pid = task
        .as_ref()
        .and_then(|task| task.get("pid"))
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok());
    let last_log = state
        .last_log_lines
        .get(&exit.reference)
        .and_then(Value::as_array)
        .and_then(|values| values.get(2))
        .cloned()
        .unwrap_or_else(|| json!(""));
    state.finished_tasks.insert(exit.reference.clone(), json!({"name":name,"exit_code":exit.exit_code,"last_log":last_log,"finished_at":sample.wall_seconds}));
    state
        .finished_task_finished_at
        .insert(exit.reference.clone(), sample.monotonic_seconds);
    state.last_log_lines.remove(&exit.reference);
    state.last_log_at.remove(&exit.reference);
    state.task_started_at.remove(&exit.reference);
    if let Some(pid) = pid {
        clear_process_display(state, pid);
        observer.forget(pid);
    }
    ReductionEffects::default()
}

fn clear_process_display(state: &mut TopState, pid: u32) {
    state.cpu_pids.remove(&pid);
    state.cpu_cache.remove(&pid);
    state.memory_cache.remove(&pid);
    state.process_identities.remove(&pid);
}

fn record_process_sample(state: &mut TopState, pid: u32, sample: ProcessSample) {
    match sample {
        ProcessSample::Live {
            identity,
            rss_bytes,
            cpu_percent,
        } => {
            if state
                .process_identities
                .get(&pid)
                .is_some_and(|known| known != &identity)
            {
                clear_process_display(state, pid);
            }
            state.process_identities.insert(pid, identity);
            state.cpu_pids.insert(pid);
            state.cpu_cache.insert(pid, cpu_percent);
            state.memory_cache.insert(pid, rss_bytes);
        }
        _ => {
            clear_process_display(state, pid);
        }
    }
}

fn validate_observe_status(
    extra: &Map<String, Value>,
) -> Result<ValidatedObserveStatus, TopMalformed> {
    let route = TopRoute::ObserveStatus;
    let mut fields = BTreeMap::new();
    let mode = match extra.get("mode") {
        None => None,
        Some(Value::String(value)) if matches!(value.as_str(), "idle" | "screencast" | "tmux") => {
            fields.insert("mode".to_owned(), json!(value));
            Some(value.clone())
        }
        Some(Value::String(_)) => {
            return Err(malformed(route, TopMalformedKind::InvalidValue("mode")));
        }
        Some(_) => return Err(malformed(route, TopMalformedKind::WrongType("mode"))),
    };
    if let Some(value) = extra.get("stream") {
        let Some(value) = value.as_str() else {
            return Err(malformed(route, TopMalformedKind::WrongType("stream")));
        };
        fields.insert("stream".to_owned(), json!(value));
    }
    for (key, leaf) in [
        ("screencast", "window_elapsed_seconds"),
        ("tmux", "captures"),
        ("audio", "threshold_hits"),
    ] {
        if let Some(value) = extra.get(key) {
            let Some(object) = value.as_object() else {
                return Err(malformed(route, TopMalformedKind::WrongType(key)));
            };
            let mut projected = Map::new();
            if let Some(value) = object.get(leaf) {
                if value.as_u64().is_none() {
                    return Err(malformed(route, TopMalformedKind::WrongType(leaf)));
                }
                projected.insert(leaf.to_owned(), value.clone());
            }
            if key == "audio"
                && let Some(value) = object.get("will_save")
            {
                if !value.is_boolean() {
                    return Err(malformed(route, TopMalformedKind::WrongType("will_save")));
                }
                projected.insert("will_save".to_owned(), value.clone());
            }
            fields.insert(key.to_owned(), Value::Object(projected));
        }
    }
    if let Some(value) = extra.get("activity") {
        let Some(object) = value.as_object() else {
            return Err(malformed(route, TopMalformedKind::WrongType("activity")));
        };
        let mut projected = Map::new();
        if let Some(value) = object.get("screen_locked") {
            if !value.is_boolean() {
                return Err(malformed(
                    route,
                    TopMalformedKind::WrongType("screen_locked"),
                ));
            }
            projected.insert("screen_locked".to_owned(), value.clone());
        }
        fields.insert("activity".to_owned(), Value::Object(projected));
    }
    for key in ["describe", "transcribe"] {
        if let Some(value) = extra.get(key) {
            let Some(object) = value.as_object() else {
                return Err(malformed(route, TopMalformedKind::WrongType(key)));
            };
            let mut projected = Map::new();
            for leaf in ["running", "queued"] {
                if let Some(value) = object.get(leaf) {
                    if !value.is_array() {
                        return Err(malformed(route, TopMalformedKind::WrongType(leaf)));
                    }
                    projected.insert(leaf.to_owned(), value.clone());
                }
            }
            fields.insert(key.to_owned(), Value::Object(projected));
        }
    }
    Ok(ValidatedObserveStatus { fields, mode })
}

fn validate_observe_observed(
    extra: &Map<String, Value>,
) -> Result<ValidatedObserved, TopMalformed> {
    let route = TopRoute::ObserveObserved;
    Ok(ValidatedObserved {
        day: required_text(extra, "day", route)?,
        segment: required_text(extra, "segment", route)?,
        duration: required_u64(extra, "duration", route)?,
    })
}

fn validate_think_started(
    extra: &Map<String, Value>,
) -> Result<ValidatedThinkStarted, TopMalformed> {
    let _ = extra;
    Ok(ValidatedThinkStarted)
}

fn validate_think_status(extra: &Map<String, Value>) -> Result<ValidatedThinkStatus, TopMalformed> {
    let route = TopRoute::ThinkStatus;
    let mut fields = BTreeMap::new();
    for key in ["mode", "day", "segment"] {
        if let Some(value) = extra.get(key) {
            if !value.is_string() {
                return Err(malformed(route, TopMalformedKind::WrongType(key)));
            }
            fields.insert(key.to_owned(), value.clone());
        }
    }
    for key in [
        "agents_total",
        "agents_completed",
        "segments_total",
        "segments_completed",
    ] {
        if let Some(value) = extra.get(key) {
            if value.as_u64().is_none() {
                return Err(malformed(route, TopMalformedKind::WrongType(key)));
            }
            fields.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(value) = extra.get("current_agents") {
        let Some(values) = value.as_array() else {
            return Err(malformed(
                route,
                TopMalformedKind::WrongType("current_agents"),
            ));
        };
        if values.iter().any(|value| !value.is_string()) {
            return Err(malformed(
                route,
                TopMalformedKind::WrongType("current_agents[]"),
            ));
        }
        fields.insert("current_agents".to_owned(), value.clone());
    }
    Ok(ValidatedThinkStatus { fields })
}

fn validate_think_completed(
    extra: &Map<String, Value>,
) -> Result<ValidatedThinkCompleted, TopMalformed> {
    let route = TopRoute::ThinkCompleted;
    let mut fields = BTreeMap::new();
    for key in ["success", "failed", "duration_ms"] {
        fields.insert(key.to_owned(), json!(required_u64(extra, key, route)?));
    }
    let names = required_array(extra, "failed_names", route)?;
    if names.iter().any(|value| !value.is_string()) {
        return Err(malformed(
            route,
            TopMalformedKind::WrongType("failed_names[]"),
        ));
    }
    fields.insert("failed_names".to_owned(), Value::Array(names.clone()));
    Ok(ValidatedThinkCompleted { fields })
}

fn commit_observe_status(
    state: &mut TopState,
    status: ValidatedObserveStatus,
    sample: &ReductionSample,
) -> ReductionEffects {
    if status.fields.is_empty() {
        return ReductionEffects::default();
    }
    state.observe_status.extend(status.fields);
    state.observe_last_ts = sample.wall_seconds;
    if status.mode.as_deref().is_some_and(|mode| mode != "idle") {
        state.displayed_mode = status.mode.expect("validated mode present");
        state.last_active_ts = sample.monotonic_seconds;
    }
    ReductionEffects::default()
}

fn commit_observe_observed(state: &mut TopState, observed: ValidatedObserved) -> ReductionEffects {
    state.recent_segments.insert(
        0,
        json!([observed.day, observed.segment, observed.duration]),
    );
    state.recent_segments.truncate(3);
    ReductionEffects::default()
}

fn commit_think_started(state: &mut TopState, _: ValidatedThinkStarted) -> ReductionEffects {
    state.think_running = true;
    state.think_status.clear();
    ReductionEffects::default()
}

fn commit_think_status(state: &mut TopState, status: ValidatedThinkStatus) -> ReductionEffects {
    if status.fields.is_empty() {
        return ReductionEffects::default();
    }
    state.think_status.extend(status.fields);
    ReductionEffects::default()
}

fn commit_think_completed(
    state: &mut TopState,
    completed: ValidatedThinkCompleted,
) -> ReductionEffects {
    state.think_running = false;
    state.think_status.clear();
    state.think_last_completed = completed.fields;
    ReductionEffects {
        refresh_brain: true,
    }
}
