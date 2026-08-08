// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-partition task admission, lifecycle, and deadline enforcement.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cap::CapResolver;
use crate::partition::Partition;
use crate::process::{
    CAP_TERMINATION_TIMEOUT, ManagedProcess, ProcessEventSink, SpawnOptions,
    TASK_QUEUE_SHUTDOWN_TIMEOUT, exit_status_for_code,
};
use crate::request::{ActiveTaskSnapshot, ExecutionRequest};

/// The status label used when deadline enforcement terminated a task.
pub const TIMEOUT_EXIT_STATUS: &str = "timeout";
const HISTORY_LIMIT: usize = 100;
const STOPPED_TICKS_THRESHOLD: u8 = 2;
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A platform process-state observation used for stopped-task enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Stopped,
    Other,
    Unknown,
}

/// Best-effort process-state source. Failures are deliberately `Unknown`.
pub trait ProcessStateProbe: Send + Sync {
    fn state(&self, pid: u32) -> ProcessState;
}

/// The native process-state source for the current target.
#[derive(Debug, Default)]
pub struct SystemProcessStateProbe;

impl ProcessStateProbe for SystemProcessStateProbe {
    fn state(&self, pid: u32) -> ProcessState {
        system_process_state(pid)
    }
}

#[cfg(target_os = "linux")]
fn system_process_state(pid: u32) -> ProcessState {
    let path = format!("/proc/{pid}/stat");
    let Ok(stat) = std::fs::read_to_string(path) else {
        return ProcessState::Unknown;
    };
    let Some(end_comm) = stat.rfind(')') else {
        return ProcessState::Unknown;
    };
    let state = stat[end_comm + 1..].split_whitespace().next();
    match state.and_then(|value| value.chars().next()) {
        Some('T' | 't') => ProcessState::Stopped,
        Some(_) => ProcessState::Other,
        None => ProcessState::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn system_process_state(pid: u32) -> ProcessState {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output();
    let Ok(output) = output else {
        return ProcessState::Unknown;
    };
    if !output.status.success() {
        return ProcessState::Unknown;
    }
    match String::from_utf8_lossy(&output.stdout)
        .trim()
        .chars()
        .next()
    {
        Some('T' | 't') => ProcessState::Stopped,
        Some(_) => ProcessState::Other,
        None => ProcessState::Unknown,
    }
}

/// iOS has neither Linux procfs nor a supported process-listing shellout.
/// Stopped-process detection is therefore explicitly unavailable on this target.
#[cfg(target_os = "ios")]
fn system_process_state(_pid: u32) -> ProcessState {
    ProcessState::Unknown
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn system_process_state(_pid: u32) -> ProcessState {
    ProcessState::Unknown
}

/// Queue lifecycle events for a caller-owned transport adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskQueueEvent {
    QueueChanged {
        partition: Partition,
        running_reference: Option<String>,
        queued_depth: usize,
        queue: Vec<QueuedTaskSnapshot>,
    },
    Started {
        partition: Partition,
        reference: String,
        command: Vec<String>,
    },
    Stopped {
        partition: Partition,
        reference: String,
        command: Vec<String>,
        exit_code: i32,
    },
}

/// Best-effort destination for queue lifecycle events.
pub trait TaskQueueEventSink: Send + Sync {
    fn emit(&self, event: TaskQueueEvent);
}

/// A queued item as visible to queue observers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedTaskSnapshot {
    pub references: Vec<String>,
    pub command: Vec<String>,
    pub day: Option<String>,
    pub scheduler_name: Option<String>,
}

/// Completion information retained for the most recent queue executions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHistoryRecord {
    pub partition: Partition,
    pub command: Vec<String>,
    pub reference: String,
    pub ended_at: SystemTime,
    pub exit_status: String,
    pub scheduler_name: Option<String>,
}

/// A status projection intentionally retaining Python's whole-second precision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    pub partition: Partition,
    pub reference: String,
    pub command: Vec<String>,
    pub duration_seconds: u64,
    pub slow: bool,
    pub stuck: bool,
}

/// A read-only active-process snapshot for a parent-death backstop.
#[derive(Clone)]
pub struct ActiveProcessHandle {
    pub reference: String,
    process: Arc<Mutex<ManagedProcess>>,
}

impl ActiveProcessHandle {
    pub fn pid(&self) -> u32 {
        self.process
            .lock()
            .expect("managed process lock poisoned")
            .pid()
    }
}

/// Result of accepting one request into the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    Pending,
    Dispatched,
    Queued,
    Coalesced,
    DuplicateQueuedReference,
}

/// Construction inputs owned by the queue rather than process-global state.
pub struct TaskQueueOptions {
    pub journal_root: PathBuf,
    pub cap_resolver: Arc<dyn CapResolver + Send + Sync>,
    pub process_state_probe: Arc<dyn ProcessStateProbe>,
    pub queue_sink: Option<Arc<dyn TaskQueueEventSink>>,
    pub process_sink: Option<Arc<dyn ProcessEventSink>>,
    pub ready: bool,
    /// Test synchronization seam invoked after Phase B and before Phase C.
    pub before_deadline_commit: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// A synchronous, per-partition task queue.
#[derive(Clone)]
pub struct TaskQueue {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    options: QueueOptions,
    state: Mutex<QueueState>,
}

struct QueueOptions {
    journal_root: PathBuf,
    cap_resolver: Arc<dyn CapResolver + Send + Sync>,
    process_state_probe: Arc<dyn ProcessStateProbe>,
    queue_sink: Option<Arc<dyn TaskQueueEventSink>>,
    process_sink: Option<Arc<dyn ProcessEventSink>>,
    before_deadline_commit: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct QueueState {
    ready: bool,
    shutdown: bool,
    running: BTreeMap<Partition, RunningSlot>,
    queues: BTreeMap<Partition, VecDeque<QueuedEntry>>,
    pending: Vec<Submission>,
    active: BTreeMap<String, ActiveEntry>,
    history: VecDeque<TaskHistoryRecord>,
    timeout_marked: BTreeSet<String>,
    stopped_ticks: BTreeMap<String, u8>,
    termination_attempts: TerminationAttemptRegistry,
}

struct RunningSlot {
    reference: String,
}

#[derive(Clone)]
struct Submission {
    partition: Partition,
    command: Vec<String>,
    reference: String,
    day: Option<String>,
    scheduler_name: Option<String>,
}

#[derive(Clone)]
struct QueuedEntry {
    references: Vec<String>,
    command: Vec<String>,
    day: Option<String>,
    scheduler_name: Option<String>,
}

#[derive(Clone)]
struct Dispatch {
    submission: Submission,
    references: Vec<String>,
}

struct ActiveEntry {
    partition: Partition,
    command: Vec<String>,
    started_at: Instant,
    started_at_unix: u64,
    process: Arc<Mutex<ManagedProcess>>,
}

type TerminationAttempt = (String, u64, Arc<Mutex<ManagedProcess>>);

#[derive(Default)]
// A future service-restart port should share this ref-keyed registry rather
// than create a separate keyspace, matching Python's shared guard.
struct TerminationAttemptRegistry {
    next_token: u64,
    by_reference: BTreeMap<String, u64>,
}

impl TerminationAttemptRegistry {
    fn begin(&mut self, reference: &str) -> Option<u64> {
        if self.by_reference.contains_key(reference) {
            return None;
        }
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.by_reference.insert(reference.to_owned(), token);
        Some(token)
    }

    fn finish(&mut self, reference: &str, token: u64) {
        if self.by_reference.get(reference) == Some(&token) {
            self.by_reference.remove(reference);
        }
    }
}

/// The private lease is the sole owner of a partition's running-slot release.
/// Its `Drop` path advances queued work even while a worker unwinds from panic.
struct WorkerLease {
    inner: Arc<QueueInner>,
    partition: Partition,
    reference: String,
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        let dispatch = finish_worker(&self.inner, &self.partition, &self.reference);
        if let Some(dispatch) = dispatch {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
    }
}

impl TaskQueue {
    pub fn new(options: TaskQueueOptions) -> Self {
        Self {
            inner: Arc::new(QueueInner {
                options: QueueOptions {
                    journal_root: options.journal_root,
                    cap_resolver: options.cap_resolver,
                    process_state_probe: options.process_state_probe,
                    queue_sink: options.queue_sink,
                    process_sink: options.process_sink,
                    before_deadline_commit: options.before_deadline_commit,
                },
                state: Mutex::new(QueueState {
                    ready: options.ready,
                    shutdown: false,
                    running: BTreeMap::new(),
                    queues: BTreeMap::new(),
                    pending: Vec::new(),
                    active: BTreeMap::new(),
                    history: VecDeque::new(),
                    timeout_marked: BTreeSet::new(),
                    stopped_ticks: BTreeMap::new(),
                    termination_attempts: TerminationAttemptRegistry::default(),
                }),
            }),
        }
    }

    pub fn submit(&self, request: ExecutionRequest) -> SubmitOutcome {
        let submission = normalize_request(request);
        let partition = submission.partition.clone();
        let (outcome, dispatch, event) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            if !state.ready {
                // AC15: this path appends every request unconditionally, so each
                // pre-ready entry has exactly one reference before set_ready drains it.
                state.pending.push(submission);
                (SubmitOutcome::Pending, None, None)
            } else if state.shutdown {
                (SubmitOutcome::Queued, None, None)
            } else {
                let (outcome, dispatch) = admit_locked(&mut state, submission);
                let event = Some(queue_changed_event(&state, &partition));
                (outcome, dispatch, event)
            }
        };
        emit_queue_event(&self.inner.options.queue_sink, event);
        if let Some(dispatch) = dispatch {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
        outcome
    }

    pub fn set_ready(&self) {
        let (dispatches, events) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            if state.ready {
                return;
            }
            state.ready = true;
            let pending = std::mem::take(&mut state.pending);
            let mut dispatches = Vec::new();
            let mut changed = BTreeSet::new();
            for submission in pending {
                let (_, dispatch) = admit_locked(&mut state, submission);
                if let Some(dispatch) = dispatch {
                    changed.insert(dispatch.submission.partition.clone());
                    dispatches.push(dispatch);
                }
            }
            let events: Vec<_> = changed
                .iter()
                .map(|partition| queue_changed_event(&state, partition))
                .collect();
            (dispatches, events)
        };
        for event in events {
            emit_queue_event(&self.inner.options.queue_sink, Some(event));
        }
        for dispatch in dispatches {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
    }

    /// Enforce effective caps. `CapResolver::cap_for` preserves zero-cap fallback
    /// to the default cap; see `cap.rs:43-50`.
    pub fn enforce_deadlines(&self, now: Instant) {
        let snapshots = {
            let state = self.inner.state.lock().expect("queue state lock poisoned");
            state
                .active
                .iter()
                .map(|(reference, active)| DeadlineSnapshot {
                    reference: reference.clone(),
                    pid: active
                        .process
                        .lock()
                        .expect("managed process lock poisoned")
                        .pid(),
                    started_at: active.started_at,
                    cap: self.inner.options.cap_resolver.cap_for(&active.partition),
                    timeout_marked: state.timeout_marked.contains(reference),
                    stopped_ticks: state.stopped_ticks.get(reference).copied().unwrap_or(0),
                })
                .collect::<Vec<_>>()
        };

        let mut proposal = DeadlineProposal::default();
        for snapshot in &snapshots {
            if now.saturating_duration_since(snapshot.started_at) > snapshot.cap {
                proposal.timeout_add.insert(snapshot.reference.clone());
                proposal.stopped_remove.insert(snapshot.reference.clone());
                proposal.terminate.insert(snapshot.reference.clone());
                continue;
            }
            if snapshot.timeout_marked {
                continue;
            }
            match self.inner.options.process_state_probe.state(snapshot.pid) {
                ProcessState::Stopped => {
                    let ticks = snapshot.stopped_ticks.saturating_add(1);
                    if ticks >= STOPPED_TICKS_THRESHOLD {
                        proposal.timeout_add.insert(snapshot.reference.clone());
                        proposal.stopped_remove.insert(snapshot.reference.clone());
                        proposal.terminate.insert(snapshot.reference.clone());
                    } else {
                        proposal
                            .stopped_set
                            .insert(snapshot.reference.clone(), ticks);
                    }
                }
                ProcessState::Other | ProcessState::Unknown => {
                    proposal.stopped_remove.insert(snapshot.reference.clone());
                }
            }
        }

        if let Some(hook) = &self.inner.options.before_deadline_commit {
            hook();
        }

        let attempts: Vec<TerminationAttempt> = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            for reference in &proposal.stopped_remove {
                state.stopped_ticks.remove(reference);
            }
            for (reference, ticks) in proposal.stopped_set {
                if state.active.contains_key(&reference) {
                    state.stopped_ticks.insert(reference, ticks);
                }
            }
            for reference in proposal.timeout_add {
                if state.active.contains_key(&reference) {
                    state.timeout_marked.insert(reference);
                }
            }
            proposal
                .terminate
                .into_iter()
                .filter_map(|reference| {
                    let process = state.active.get(&reference)?.process.clone();
                    let token = state.termination_attempts.begin(&reference)?;
                    Some((reference, token, process))
                })
                .collect::<Vec<_>>()
        };
        for (reference, token, process) in attempts {
            start_termination(
                Arc::clone(&self.inner),
                reference,
                token,
                process,
                CAP_TERMINATION_TIMEOUT,
            );
        }
    }

    /// Stops dispatch advancement, leaves queued/pending entries inert, and
    /// returns the active-process snapshot size from shutdown start.
    pub fn shutdown(&self) -> usize {
        let (active_count, attempts): (usize, Vec<TerminationAttempt>) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            state.shutdown = true;
            let active = state
                .active
                .iter()
                .map(|(reference, active)| (reference.clone(), Arc::clone(&active.process)))
                .collect::<Vec<_>>();
            let active_count = active.len();
            let attempts = active
                .into_iter()
                .filter_map(|(reference, process)| {
                    let token = state.termination_attempts.begin(&reference)?;
                    Some((reference, token, process))
                })
                .collect();
            (active_count, attempts)
        };
        let mut threads = Vec::new();
        for (reference, token, process) in attempts {
            let inner = Arc::clone(&self.inner);
            if let Ok(handle) = thread::Builder::new().spawn(move || {
                terminate_process(
                    &inner,
                    &reference,
                    token,
                    process,
                    TASK_QUEUE_SHUTDOWN_TIMEOUT,
                );
            }) {
                threads.push(handle);
            }
        }
        for thread in threads {
            let _ = thread.join();
        }
        active_count
    }

    pub fn collect_task_status(&self, now: Instant) -> Vec<TaskStatus> {
        let state = self.inner.state.lock().expect("queue state lock poisoned");
        state
            .active
            .iter()
            .map(|(reference, active)| {
                let duration_seconds = now.saturating_duration_since(active.started_at).as_secs();
                let cap_seconds = self
                    .inner
                    .options
                    .cap_resolver
                    .cap_for(&active.partition)
                    .as_secs();
                TaskStatus {
                    partition: active.partition.clone(),
                    reference: reference.clone(),
                    command: active.command.clone(),
                    duration_seconds,
                    slow: duration_seconds.saturating_mul(4) >= cap_seconds.saturating_mul(3),
                    stuck: duration_seconds > cap_seconds,
                }
            })
            .collect()
    }

    pub fn collect_queue_counts(&self) -> BTreeMap<String, usize> {
        let state = self.inner.state.lock().expect("queue state lock poisoned");
        let mut counts = state
            .queues
            .iter()
            .filter(|(_, queue)| !queue.is_empty())
            .map(|(partition, queue)| (partition.as_str().to_owned(), queue.len()))
            .collect::<BTreeMap<_, _>>();
        if !state.pending.is_empty() {
            counts.insert("pending".to_owned(), state.pending.len());
        }
        counts
    }

    pub fn get_active_by_cmd_name(&self, partition: &Partition) -> Option<ActiveTaskSnapshot> {
        let state = self.inner.state.lock().expect("queue state lock poisoned");
        state
            .active
            .iter()
            .find(|(_, active)| &active.partition == partition)
            .map(|(reference, active)| ActiveTaskSnapshot {
                reference: reference.clone(),
                cmd: Some(active.command.clone()),
                started_at: Some(active.started_at_unix),
            })
    }

    pub fn active_process_handles(&self) -> Vec<ActiveProcessHandle> {
        let state = self.inner.state.lock().expect("queue state lock poisoned");
        state
            .active
            .iter()
            .map(|(reference, active)| ActiveProcessHandle {
                reference: reference.clone(),
                process: Arc::clone(&active.process),
            })
            .collect()
    }

    pub fn history(&self) -> Vec<TaskHistoryRecord> {
        self.inner
            .state
            .lock()
            .expect("queue state lock poisoned")
            .history
            .iter()
            .cloned()
            .collect()
    }
}

fn normalize_request(request: ExecutionRequest) -> Submission {
    match request {
        ExecutionRequest::Bus(request) => Submission {
            partition: request.cmd.partition(),
            command: request.cmd.as_wire().to_vec(),
            reference: request.reference,
            day: request.day,
            scheduler_name: request.scheduler_name,
        },
        ExecutionRequest::Scheduled(request) => Submission {
            partition: request.cmd.partition(),
            command: request.cmd.as_wire().to_vec(),
            reference: request.reference,
            day: request.day,
            scheduler_name: Some(request.scheduler_name),
        },
    }
}

fn admit_locked(
    state: &mut QueueState,
    submission: Submission,
) -> (SubmitOutcome, Option<Dispatch>) {
    if state.running.contains_key(&submission.partition) {
        let queue = state
            .queues
            .entry(submission.partition.clone())
            .or_default();
        if let Some(entry) = queue
            .iter_mut()
            .find(|entry| entry.command == submission.command)
        {
            if entry.references.contains(&submission.reference) {
                return (SubmitOutcome::DuplicateQueuedReference, None);
            }
            entry.references.push(submission.reference);
            return (SubmitOutcome::Coalesced, None);
        }
        queue.push_back(QueuedEntry {
            references: vec![submission.reference.clone()],
            command: submission.command.clone(),
            day: submission.day.clone(),
            scheduler_name: submission.scheduler_name.clone(),
        });
        return (SubmitOutcome::Queued, None);
    }
    state.running.insert(
        submission.partition.clone(),
        RunningSlot {
            reference: submission.reference.clone(),
        },
    );
    (
        SubmitOutcome::Dispatched,
        Some(Dispatch {
            references: vec![submission.reference.clone()],
            submission,
        }),
    )
}

fn start_dispatch(inner: Arc<QueueInner>, dispatch: Dispatch) {
    let partition = dispatch.submission.partition.clone();
    let reference = dispatch.submission.reference.clone();
    let worker_inner = Arc::clone(&inner);
    let rollback = dispatch.clone();
    let spawned = thread::Builder::new().spawn(move || {
        let _lease = WorkerLease {
            inner: worker_inner.clone(),
            partition,
            reference,
        };
        run_worker(worker_inner, dispatch);
    });
    if spawned.is_err() {
        let next = finish_worker(
            &inner,
            &rollback.submission.partition,
            &rollback.submission.reference,
        );
        if let Some(next) = next {
            start_dispatch(inner, next);
        }
    }
}

fn run_worker(inner: Arc<QueueInner>, dispatch: Dispatch) {
    let primary = dispatch.submission.reference.clone();
    let process = ManagedProcess::spawn(
        dispatch.submission.command.clone(),
        SpawnOptions {
            journal_root: inner.options.journal_root.clone(),
            reference: primary.clone(),
            day: dispatch.submission.day.clone(),
            sink: inner.options.process_sink.clone(),
        },
    );
    let Ok(process) = process else {
        record_completion(&inner, &dispatch, -1, "error".to_owned());
        return;
    };
    let process = Arc::new(Mutex::new(process));
    let started_at = Instant::now();
    let started_at_unix = unix_seconds();
    {
        let mut state = inner.state.lock().expect("queue state lock poisoned");
        state.active.insert(
            primary.clone(),
            ActiveEntry {
                partition: dispatch.submission.partition.clone(),
                command: dispatch.submission.command.clone(),
                started_at,
                started_at_unix,
                process: Arc::clone(&process),
            },
        );
    }
    emit_queue_event(
        &inner.options.queue_sink,
        Some(TaskQueueEvent::Started {
            partition: dispatch.submission.partition.clone(),
            reference: primary.clone(),
            command: dispatch.submission.command.clone(),
        }),
    );
    let exit_code = loop {
        let result = process
            .lock()
            .expect("managed process lock poisoned")
            .poll();
        match result {
            Ok(Some(code)) => break code,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(_) => break -1,
        }
    };
    process
        .lock()
        .expect("managed process lock poisoned")
        .cleanup();
    record_completion(
        &inner,
        &dispatch,
        exit_code,
        exit_status_for_code(exit_code).to_owned(),
    );
}

fn record_completion(
    inner: &QueueInner,
    dispatch: &Dispatch,
    exit_code: i32,
    default_status: String,
) {
    let status = {
        let mut state = inner.state.lock().expect("queue state lock poisoned");
        state.active.remove(&dispatch.submission.reference);
        state.stopped_ticks.remove(&dispatch.submission.reference);
        state
            .termination_attempts
            .by_reference
            .remove(&dispatch.submission.reference);
        let status = if state.timeout_marked.remove(&dispatch.submission.reference) {
            TIMEOUT_EXIT_STATUS.to_owned()
        } else {
            default_status
        };
        if state.history.len() == HISTORY_LIMIT {
            state.history.pop_front();
        }
        state.history.push_back(TaskHistoryRecord {
            partition: dispatch.submission.partition.clone(),
            command: dispatch.submission.command.clone(),
            reference: dispatch.submission.reference.clone(),
            ended_at: SystemTime::now(),
            exit_status: status.clone(),
            scheduler_name: dispatch.submission.scheduler_name.clone(),
        });
        status
    };
    let _ = status;
    for reference in &dispatch.references {
        emit_queue_event(
            &inner.options.queue_sink,
            Some(TaskQueueEvent::Stopped {
                partition: dispatch.submission.partition.clone(),
                reference: reference.clone(),
                command: dispatch.submission.command.clone(),
                exit_code,
            }),
        );
    }
}

fn finish_worker(inner: &QueueInner, partition: &Partition, reference: &str) -> Option<Dispatch> {
    let (dispatch, event) = {
        let mut state = inner.state.lock().expect("queue state lock poisoned");
        if state
            .running
            .get(partition)
            .map(|slot| slot.reference.as_str())
            != Some(reference)
        {
            return None;
        }
        state.running.remove(partition);
        if state.shutdown {
            return None;
        }
        let dispatch = state
            .queues
            .get_mut(partition)
            .and_then(VecDeque::pop_front)
            .map(|entry| {
                let submission = Submission {
                    partition: partition.clone(),
                    command: entry.command,
                    reference: entry.references[0].clone(),
                    day: entry.day,
                    scheduler_name: entry.scheduler_name,
                };
                state.running.insert(
                    partition.clone(),
                    RunningSlot {
                        reference: submission.reference.clone(),
                    },
                );
                Dispatch {
                    references: entry.references,
                    submission,
                }
            });
        if state.queues.get(partition).is_some_and(VecDeque::is_empty) {
            state.queues.remove(partition);
        }
        let event = Some(queue_changed_event(&state, partition));
        (dispatch, event)
    };
    emit_queue_event(&inner.options.queue_sink, event);
    dispatch
}

fn queue_changed_event(state: &QueueState, partition: &Partition) -> TaskQueueEvent {
    let queue: Vec<_> = state
        .queues
        .get(partition)
        .map(|queue| {
            queue
                .iter()
                .map(|entry| QueuedTaskSnapshot {
                    references: entry.references.clone(),
                    command: entry.command.clone(),
                    day: entry.day.clone(),
                    scheduler_name: entry.scheduler_name.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    TaskQueueEvent::QueueChanged {
        partition: partition.clone(),
        running_reference: state
            .running
            .get(partition)
            .map(|slot| slot.reference.clone()),
        queued_depth: queue.len(),
        queue,
    }
}

fn emit_queue_event(sink: &Option<Arc<dyn TaskQueueEventSink>>, event: Option<TaskQueueEvent>) {
    if let (Some(sink), Some(event)) = (sink, event) {
        sink.emit(event);
    }
}

struct DeadlineSnapshot {
    reference: String,
    pid: u32,
    started_at: Instant,
    cap: Duration,
    timeout_marked: bool,
    stopped_ticks: u8,
}

#[derive(Default)]
struct DeadlineProposal {
    timeout_add: BTreeSet<String>,
    stopped_set: BTreeMap<String, u8>,
    stopped_remove: BTreeSet<String>,
    terminate: BTreeSet<String>,
}

fn start_termination(
    inner: Arc<QueueInner>,
    reference: String,
    token: u64,
    process: Arc<Mutex<ManagedProcess>>,
    timeout: Duration,
) {
    let thread_inner = Arc::clone(&inner);
    let thread_reference = reference.clone();
    if thread::Builder::new()
        .spawn(move || terminate_process(&thread_inner, &thread_reference, token, process, timeout))
        .is_err()
    {
        inner
            .state
            .lock()
            .expect("queue state lock poisoned")
            .termination_attempts
            .finish(&reference, token);
    }
}

fn terminate_process(
    inner: &QueueInner,
    reference: &str,
    token: u64,
    process: Arc<Mutex<ManagedProcess>>,
    timeout: Duration,
) {
    let _ = process
        .lock()
        .expect("managed process lock poisoned")
        .terminate(timeout);
    inner
        .state
        .lock()
        .expect("queue state lock poisoned")
        .termination_attempts
        .finish(reference, token);
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
