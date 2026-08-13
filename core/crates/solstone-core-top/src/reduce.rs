// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumDiscontinuity, CallosumEnvelope, CallosumReceiveEvent};

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

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TopReduceError {
    #[error("service is missing pid")]
    MissingServicePid,
    #[error("services must be an array")]
    ServicesWrongType,
    #[error("queues must be an object")]
    QueuesWrongType,
    #[error("queued count must be numeric")]
    QueueCountWrongType,
}

/// Apply a decoded envelope without reading the clock or process table itself.
pub fn reduce_envelope(
    state: &mut TopState,
    envelope: &CallosumEnvelope,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> Result<ReductionEffects, TopReduceError> {
    match (envelope.tract.as_str(), envelope.event.as_str()) {
        ("supervisor", "status") => reduce_status(state, &envelope.extra, sample, observer),
        ("supervisor", "restarting" | "started" | "stopped") => {
            if let Some(service) = text(&envelope.extra, "service") {
                state.service_status.insert(
                    service.to_owned(),
                    (envelope.event.clone(), sample.wall_seconds),
                );
            }
            Ok(ReductionEffects::default())
        }
        ("supervisor", "queue") => reduce_queue(state, &envelope.extra),
        ("logs", "exec") => {
            reduce_exec(state, &envelope.extra, sample, observer);
            Ok(ReductionEffects::default())
        }
        ("logs", "line") => {
            reduce_line(state, &envelope.extra, sample, observer);
            Ok(ReductionEffects::default())
        }
        ("logs", "exit") => {
            reduce_exit(state, &envelope.extra, sample);
            Ok(ReductionEffects::default())
        }
        ("observe", "status") => {
            reduce_observe_status(state, &envelope.extra, sample);
            Ok(ReductionEffects::default())
        }
        ("observe", "observed") => {
            reduce_observed(state, &envelope.extra);
            Ok(ReductionEffects::default())
        }
        ("think", "started") => {
            state.think_running = true;
            state.think_status.clear();
            Ok(ReductionEffects::default())
        }
        ("think", "status") => {
            state.think_status.extend(envelope.extra.clone());
            Ok(ReductionEffects::default())
        }
        ("think", "completed") => {
            state.think_running = false;
            state.think_status.clear();
            state.think_last_completed = envelope.extra.clone().into_iter().collect();
            Ok(ReductionEffects {
                refresh_brain: true,
            })
        }
        _ => Ok(ReductionEffects::default()),
    }
}

/// Apply continuity changes before reducing a tagged envelope.
pub fn apply_receive_event(
    state: &mut TopState,
    event: &CallosumReceiveEvent,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> Result<ReductionEffects, TopReduceError> {
    match event {
        CallosumReceiveEvent::Envelope {
            generation,
            envelope,
        } => {
            state.continuity.generation = *generation;
            let effects = reduce_envelope(state, envelope, sample, observer)?;
            if envelope.tract == "supervisor"
                && matches!(
                    envelope.event.as_str(),
                    "restarting" | "started" | "stopped"
                )
                && let Some(service) = text(&envelope.extra, "service")
            {
                let _ = acknowledge_restart(
                    state,
                    service,
                    text(&envelope.extra, "restart_id"),
                    *generation,
                    &envelope.event,
                    sample.monotonic_seconds,
                );
            }
            match (envelope.tract.as_str(), envelope.event.as_str()) {
                ("supervisor", "status") => state.continuity.supervisor_gap = false,
                ("logs", "exec" | "line") => state.continuity.task_gap = false,
                ("observe", "status") => state.continuity.observe_gap = false,
                ("think", "started" | "status") => state.continuity.think_gap = false,
                _ => {}
            }
            Ok(effects)
        }
        CallosumReceiveEvent::Discontinuity { generation, reason } => {
            let previous_generation = state.continuity.generation;
            state.continuity.generation = *generation;
            if *generation != previous_generation {
                let _ = fail_discontinuous_restarts(state, *generation, sample.monotonic_seconds);
            }
            if *generation != previous_generation
                || !matches!(reason, CallosumDiscontinuity::Connected)
            {
                state.continuity.supervisor_gap = true;
                state.continuity.task_gap = true;
                state.continuity.observe_gap = true;
                state.continuity.think_gap = true;
                state.services.clear();
                state.crashed.clear();
                state.command_queues.clear();
                state.cpu_cache.clear();
                state.memory_cache.clear();
                state.cpu_pids.clear();
                state.running_tasks.clear();
                state.task_started_at.clear();
                state.last_log_lines.clear();
                state.last_log_at.clear();
                state.observe_status.clear();
                state.observe_last_ts = 0.0;
                state.think_running = false;
                state.think_status.clear();
                for attempt in state.restart_attempts.values_mut() {
                    if matches!(
                        attempt.phase,
                        crate::RestartPhase::Pending | crate::RestartPhase::Restarting
                    ) {
                        attempt.phase =
                            crate::RestartPhase::Failed(crate::RestartFailure::Discontinuity);
                        attempt.phase_at = sample.monotonic_seconds;
                        attempt.terminal_at = Some(sample.monotonic_seconds);
                    }
                }
            }
            Ok(ReductionEffects::default())
        }
    }
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
        .finished_tasks
        .iter()
        .filter_map(|(reference, task)| {
            task.get("finished_at")
                .and_then(Value::as_f64)
                .filter(|at| sample.wall_seconds - at > STATUS_TIMEOUT_SECONDS)
                .map(|_| reference.clone())
        })
        .collect();
    for reference in expired {
        state.finished_tasks.remove(&reference);
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
        state.cpu_pids.remove(&pid);
        state.cpu_cache.remove(&pid);
        state.memory_cache.remove(&pid);
        state.task_started_at.remove(&reference);
        state.last_log_at.remove(&reference);
        state.finished_tasks.insert(reference, json!({"name":name,"exit_code":Value::Null,"last_log":last_log,"finished_at":sample.wall_seconds}));
    }
}

fn reduce_status(
    state: &mut TopState,
    extra: &Map<String, Value>,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) -> Result<ReductionEffects, TopReduceError> {
    let services = match extra.get("services") {
        None => Vec::new(),
        Some(Value::Array(values)) => values.clone(),
        Some(_) => return Err(TopReduceError::ServicesWrongType),
    };
    for service in &services {
        if service.get("pid").and_then(Value::as_u64).is_none() {
            return Err(TopReduceError::MissingServicePid);
        }
    }
    let crashed = match extra.get("crashed") {
        Some(Value::Array(values)) => values.clone(),
        _ => Vec::new(),
    };
    if let Some(queues) = extra.get("queues") {
        let Some(queues) = queues.as_object() else {
            return Err(TopReduceError::QueuesWrongType);
        };
        state.command_queues = queues
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
    }
    state.services = services;
    state.crashed = crashed;
    state.selected = state.selected.min(state.services.len().saturating_sub(1));
    state.cpu_pids.clear();
    state.memory_cache.clear();
    let service_pids = state
        .services
        .iter()
        .map(|service| service["pid"].as_u64().expect("pid checked") as u32)
        .collect::<Vec<_>>();
    for pid in service_pids {
        record_process_sample(state, pid, observer.sample(pid, sample.monotonic_seconds));
    }
    Ok(ReductionEffects::default())
}

fn reduce_queue(
    state: &mut TopState,
    extra: &Map<String, Value>,
) -> Result<ReductionEffects, TopReduceError> {
    let Some(command) = text(extra, "command") else {
        return Ok(ReductionEffects::default());
    };
    let queued = extra.get("queued").cloned().unwrap_or_else(|| json!(0));
    let Some(count) = queued.as_f64() else {
        return Err(TopReduceError::QueueCountWrongType);
    };
    if count > 0.0 {
        state.command_queues.insert(command.to_owned(), queued);
    } else {
        state.command_queues.remove(command);
    }
    Ok(ReductionEffects::default())
}

fn reduce_exec(
    state: &mut TopState,
    extra: &Map<String, Value>,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) {
    let (Some(reference), Some(name), Some(pid)) = (
        text(extra, "ref"),
        text(extra, "name"),
        extra.get("pid").and_then(Value::as_u64),
    ) else {
        return;
    };
    state.running_tasks.insert(reference.to_owned(), json!({"ref": reference, "name": name, "pid": pid, "cmd": extra.get("cmd").cloned().unwrap_or_else(|| json!([])), "start_time": sample.wall_datetime}));
    state
        .task_started_at
        .insert(reference.to_owned(), sample.monotonic_seconds);
    let pid = pid as u32;
    record_process_sample(state, pid, observer.sample(pid, sample.monotonic_seconds));
}

fn reduce_line(
    state: &mut TopState,
    extra: &Map<String, Value>,
    sample: &ReductionSample,
    observer: &mut dyn ProcessObserver,
) {
    let Some(reference) = text(extra, "ref") else {
        return;
    };
    if !state.running_tasks.contains_key(reference)
        && let (Some(name), Some(pid)) = (
            text(extra, "name"),
            extra.get("pid").and_then(Value::as_u64),
        )
    {
        state.running_tasks.insert(reference.to_owned(), json!({"ref": reference, "name": name, "pid": pid, "cmd": [], "start_time": sample.wall_datetime}));
        state
            .task_started_at
            .insert(reference.to_owned(), sample.monotonic_seconds);
        record_process_sample(
            state,
            pid as u32,
            observer.sample(pid as u32, sample.monotonic_seconds),
        );
    }
    state.last_log_lines.insert(
        reference.to_owned(),
        json!([
            sample.wall_datetime,
            extra
                .get("stream")
                .cloned()
                .unwrap_or_else(|| json!("stdout")),
            extra.get("line").cloned().unwrap_or_else(|| json!(""))
        ]),
    );
    state
        .last_log_at
        .insert(reference.to_owned(), sample.wall_seconds);
}

fn reduce_exit(state: &mut TopState, extra: &Map<String, Value>, sample: &ReductionSample) {
    let Some(reference) = text(extra, "ref") else {
        return;
    };
    let task = state.running_tasks.remove(reference);
    let name = task
        .as_ref()
        .and_then(|task| task.get("name"))
        .and_then(Value::as_str)
        .or_else(|| text(extra, "name"))
        .unwrap_or("unknown");
    let pid = task
        .as_ref()
        .and_then(|task| task.get("pid"))
        .and_then(Value::as_u64)
        .map(|pid| pid as u32);
    let last_log = state
        .last_log_lines
        .get(reference)
        .and_then(Value::as_array)
        .and_then(|values| values.get(2))
        .cloned()
        .unwrap_or_else(|| json!(""));
    state.finished_tasks.insert(reference.to_owned(), json!({"name": name, "exit_code": extra.get("exit_code").cloned().unwrap_or(Value::Null), "last_log": last_log, "finished_at": sample.wall_seconds}));
    state.last_log_lines.remove(reference);
    state.last_log_at.remove(reference);
    state.task_started_at.remove(reference);
    if let Some(pid) = pid {
        state.cpu_pids.remove(&pid);
        state.cpu_cache.remove(&pid);
        state.memory_cache.remove(&pid);
    }
}

fn record_process_sample(state: &mut TopState, pid: u32, sample: ProcessSample) {
    match sample {
        ProcessSample::Live {
            rss_bytes,
            cpu_percent,
        } => {
            state.cpu_pids.insert(pid);
            state.cpu_cache.insert(pid, cpu_percent);
            state.memory_cache.insert(pid, rss_bytes);
        }
        _ => {
            state.cpu_pids.remove(&pid);
            state.cpu_cache.remove(&pid);
            state.memory_cache.remove(&pid);
        }
    }
}

fn reduce_observe_status(
    state: &mut TopState,
    extra: &Map<String, Value>,
    sample: &ReductionSample,
) {
    state.observe_status.extend(extra.clone());
    state.observe_last_ts = sample.wall_seconds;
    if text(extra, "mode").is_some_and(|mode| mode != "idle") {
        state.displayed_mode = text(extra, "mode").unwrap_or("idle").to_owned();
        state.last_active_ts = sample.wall_seconds;
    }
}

fn reduce_observed(state: &mut TopState, extra: &Map<String, Value>) {
    let (Some(day), Some(segment)) = (text(extra, "day"), text(extra, "segment")) else {
        return;
    };
    state.recent_segments.insert(
        0,
        json!([
            day,
            segment,
            extra.get("duration").cloned().unwrap_or_else(|| json!(0))
        ]),
    );
    state.recent_segments.truncate(3);
}

fn text<'a>(extra: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    extra.get(key).and_then(Value::as_str)
}
