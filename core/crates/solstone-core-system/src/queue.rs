// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-partition task admission, lifecycle, and deadline enforcement.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cap::CapResolver;
#[cfg(not(test))]
use crate::catchup::admit_daily_catchup;
use crate::catchup::{
    CatchupError, DailyCatchupAdmission, DailyCatchupOutcome,
    record_daily_catchup_admission_failure, record_daily_catchup_outcome,
};
#[cfg(test)]
use crate::catchup::{admit_daily_catchup_with_capability, catchup_marker_capability};
use crate::partition::Partition;
use crate::process::{
    CAP_TERMINATION_TIMEOUT, Disposition, ExecutionState, InspectResult, LaunchAuthority,
    LaunchError, ManagedProcess, ProcessEventSink, ProcessInstanceSource, SpawnError, SpawnOptions,
    SystemProcessInstanceSource, TASK_QUEUE_SHUTDOWN_TIMEOUT, TerminationError, TerminationOutcome,
    exit_status_for_code, launch_managed,
};
use crate::request::{ActiveTaskSnapshot, DailyCatchupProvenance, ExecutionRequest};

/// The byte-identical Python status label consumed downstream for deadline termination.
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

/// Best-effort process-state source. Injection makes lock-discipline tests deterministic;
/// failures are deliberately `Unknown`.
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

fn system_process_state(pid: u32) -> ProcessState {
    match SystemProcessInstanceSource.inspect(pid) {
        InspectResult::Present {
            execution: ExecutionState::Stopped,
            ..
        } => ProcessState::Stopped,
        InspectResult::Present {
            execution: ExecutionState::Running,
            ..
        } => ProcessState::Other,
        InspectResult::Absent | InspectResult::Unverifiable => ProcessState::Unknown,
    }
}

/// Queue lifecycle events for a caller-owned transport adapter.
///
/// Started is primary-reference-only, while Stopped fans out to coalesced references,
/// preserving the supervisor's per-reference completion contract.
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
    pub cap_seconds: u64,
    pub slow: bool,
    pub stuck: bool,
}

/// One coherent queue-status read captured under one queue-state lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQueueStatusSnapshot {
    pub tasks: Vec<TaskStatus>,
    pub recent_tasks: Vec<TaskHistoryRecord>,
    pub queues: BTreeMap<String, usize>,
}

/// Summary of a task-queue shutdown captured from the active snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskQueueShutdownReport {
    pub active_count: usize,
    pub forced: bool,
}

trait QueueProcess: Send {
    fn pid(&self) -> u32;
    fn poll(&mut self) -> io::Result<Option<i32>>;
    fn terminate_exact(
        &mut self,
        timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError>;
    fn terminate_exact_until(
        &mut self,
        deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError>;
    fn cleanup(&mut self);
    fn cleanup_until(&mut self, deadline: Instant) -> bool;
    fn detach_after_bounded_shutdown(&mut self);
}

struct ManagedQueueProcess(LaunchAuthority);

impl QueueProcess for ManagedQueueProcess {
    fn pid(&self) -> u32 {
        self.0.pid()
    }

    fn poll(&mut self) -> io::Result<Option<i32>> {
        self.0.poll()
    }

    fn terminate_exact(
        &mut self,
        timeout: Duration,
    ) -> Result<TerminationOutcome, TerminationError> {
        match self.0.terminate_exact(timeout) {
            Ok(()) => Ok(TerminationOutcome::Graceful { exit_code: None }),
            Err(LaunchError::Terminate(error)) => Err(TerminationError::Io(error)),
            Err(error) => Err(TerminationError::Io(io::Error::other(error))),
        }
    }

    fn terminate_exact_until(
        &mut self,
        deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError> {
        match self.0.terminate_exact_until(deadline) {
            Ok(()) => Ok(TerminationOutcome::Graceful { exit_code: None }),
            Err(LaunchError::Terminate(error)) => Err(TerminationError::Io(error)),
            Err(error) => Err(TerminationError::Io(io::Error::other(error))),
        }
    }

    fn cleanup(&mut self) {
        self.0.cleanup();
    }

    fn cleanup_until(&mut self, deadline: Instant) -> bool {
        self.0.cleanup_until(deadline)
    }

    fn detach_after_bounded_shutdown(&mut self) {
        self.0.detach_after_bounded_shutdown();
    }
}

type QueueProcessHandle = Arc<Mutex<Box<dyn QueueProcess>>>;
type QueueProcessSpawner = Arc<
    dyn Fn(Vec<String>, SpawnOptions, Duration) -> Result<QueueProcessHandle, SpawnError>
        + Send
        + Sync,
>;

#[cfg(test)]
type CatchupAdmissionCapability = Arc<dyn Fn() -> Result<(), CatchupError> + Send + Sync>;

#[cfg(test)]
type WorkerThreadSpawner =
    Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> + Send + Sync>;

fn spawn_managed_queue_process(
    command: Vec<String>,
    options: SpawnOptions,
    timeout: Duration,
) -> Result<QueueProcessHandle, SpawnError> {
    let authority = match launch_managed(Disposition::IndependentBoundedHelper { timeout }, || {
        ManagedProcess::spawn_exact(command, options)
    }) {
        Ok(authority) => authority,
        Err(LaunchError::SpawnManaged(error)) => return Err(error),
        Err(LaunchError::CapabilityUnavailable { needed }) => {
            return Err(SpawnError::Spawn(io::Error::other(format!(
                "independent launch requires {needed}"
            ))));
        }
        Err(error) => {
            unreachable!("launch_managed(IndependentBoundedHelper) cannot fail with {error}")
        }
    };
    Ok(Arc::new(Mutex::new(Box::new(ManagedQueueProcess(
        authority,
    )))))
}

/// A read-only active-process snapshot for a parent-death backstop.
#[derive(Clone)]
pub struct ActiveProcessHandle {
    pub reference: String,
    process: QueueProcessHandle,
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
    Rejected,
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
    /// Environment merged into every queued task's spawn, e.g. an inherited
    /// speakers-analyze generation (see
    /// `solstone-core-transcribe::SpeakersAnalyzeGeneration`) so a scheduled
    /// `journal think --day` catchup task borrows the supervisor's held
    /// generation. Tasks that do not consult these keys ignore them.
    pub child_environment: BTreeMap<OsString, OsString>,
}

/// A synchronous, per-partition task queue.
///
/// This stays on `std::thread` because ManagedProcess is synchronous and this crate
/// has no async-I/O need; one flat module keeps its tightly coupled state transitions visible.
#[derive(Clone)]
pub struct TaskQueue {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    options: QueueOptions,
    state: Mutex<QueueState>,
    reaped: Condvar,
    worker_spawner: Mutex<QueueProcessSpawner>,
    #[cfg(test)]
    catchup_admission_capability: Mutex<CatchupAdmissionCapability>,
    #[cfg(test)]
    worker_thread_spawner: Mutex<WorkerThreadSpawner>,
    #[cfg(test)]
    worker_threads: Mutex<Vec<thread::JoinHandle<()>>>,
    #[cfg(test)]
    worker_threads_changed: Condvar,
}

struct QueueOptions {
    journal_root: PathBuf,
    cap_resolver: Arc<dyn CapResolver + Send + Sync>,
    process_state_probe: Arc<dyn ProcessStateProbe>,
    queue_sink: Option<Arc<dyn TaskQueueEventSink>>,
    process_sink: Option<Arc<dyn ProcessEventSink>>,
    before_deadline_commit: Option<Arc<dyn Fn() + Send + Sync>>,
    child_environment: BTreeMap<OsString, OsString>,
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
    cap: Duration,
    command: Vec<String>,
    reference: String,
    day: Option<String>,
    scheduler_name: Option<String>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
}

#[derive(Clone)]
struct QueuedEntry {
    references: Vec<String>,
    cap: Duration,
    command: Vec<String>,
    day: Option<String>,
    scheduler_name: Option<String>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
}

#[derive(Clone)]
struct Dispatch {
    submission: Submission,
    references: Vec<String>,
    daily_catchup_admission: Option<DailyCatchupAdmission>,
}

struct ActiveEntry {
    partition: Partition,
    cap: Duration,
    command: Vec<String>,
    started_at: Instant,
    started_at_unix: u64,
    pid: u32,
    process: QueueProcessHandle,
}

type TerminationAttempt = (String, u64, QueueProcessHandle);
type ShutdownSnapshot = (String, QueueProcessHandle);

// This guard is separate from deadline detection so repeated enforcement ticks start
// only one termination attempt per ref. A future service-restart port should share it.
#[derive(Default)]
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
                    child_environment: options.child_environment,
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
                reaped: Condvar::new(),
                worker_spawner: Mutex::new(Arc::new(spawn_managed_queue_process)),
                #[cfg(test)]
                catchup_admission_capability: Mutex::new(Arc::new(catchup_marker_capability)),
                #[cfg(test)]
                worker_thread_spawner: Mutex::new(Arc::new(|worker| {
                    thread::Builder::new().spawn(worker)
                })),
                #[cfg(test)]
                worker_threads: Mutex::new(Vec::new()),
                #[cfg(test)]
                worker_threads_changed: Condvar::new(),
            }),
        }
    }

    pub fn submit(&self, request: ExecutionRequest) -> SubmitOutcome {
        let submission = normalize_request(request, self.inner.options.cap_resolver.as_ref());
        let partition = submission.partition.clone();
        let (outcome, dispatch, event) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            if !state.ready {
                // AC15: this path appends every request unconditionally, so each
                // pre-ready entry has exactly one reference before set_ready drains it.
                state.pending.push(submission);
                (SubmitOutcome::Pending, None, None)
            } else if state.shutdown {
                (SubmitOutcome::Rejected, None, None)
            } else {
                let (outcome, dispatch) = admit_locked(&mut state, submission);
                let event = Some(queue_changed_event(&state, &partition));
                (outcome, dispatch, event)
            }
        };
        if let Some(dispatch) = dispatch {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
        emit_queue_event(&self.inner.options.queue_sink, event);
        outcome
    }

    pub fn set_ready(&self) {
        let (dispatches, events) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            if state.ready {
                return;
            }
            state.ready = true;
            // Shutdown leaves accepted pre-ready entries inert: readiness must not revive them.
            if state.shutdown {
                return;
            }
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

    /// Enforce the effective budget retained when each task was admitted.
    ///
    /// Phase A snapshots under the queue lock, Phase B decides cap-first then stopped
    /// outcomes without it, Phase C commits guarded additions/unconditional removals,
    /// and Phase D starts termination threads unlocked. Phase C marks timeout before
    /// Phase D can terminate, preserving the timeout history label for a fast exit.
    pub fn enforce_deadlines(&self, now: Instant) {
        let snapshots = {
            let state = self.inner.state.lock().expect("queue state lock poisoned");
            state
                .active
                .iter()
                .map(|(reference, active)| DeadlineSnapshot {
                    reference: reference.clone(),
                    pid: active.pid,
                    started_at: active.started_at,
                    cap: active.cap,
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
    /// reports the active-process snapshot and any forced worker termination.
    pub fn shutdown(&self) -> TaskQueueShutdownReport {
        let (active_count, snapshot): (usize, Vec<ShutdownSnapshot>) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            state.shutdown = true;
            let active = state
                .active
                .iter()
                .map(|(reference, active)| (reference.clone(), Arc::clone(&active.process)))
                .collect::<Vec<_>>();
            let active_count = active.len();
            (active_count, active)
        };
        let mut threads = Vec::new();
        let references = snapshot
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        for (_, process) in snapshot {
            if let Ok(handle) = thread::Builder::new().spawn(move || {
                matches!(
                    process
                        .lock()
                        .expect("managed process lock poisoned")
                        .terminate_exact(TASK_QUEUE_SHUTDOWN_TIMEOUT),
                    Err(TerminationError::ParentGraceTimeout)
                )
            }) {
                threads.push(handle);
            }
        }
        let mut forced = false;
        for thread in threads {
            forced |= thread.join().unwrap_or(false);
        }
        let deadline = Instant::now() + TASK_QUEUE_SHUTDOWN_TIMEOUT;
        let mut state = self.inner.state.lock().expect("queue state lock poisoned");
        while references
            .iter()
            .any(|reference| state.active.contains_key(reference))
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, timeout) = self
                .inner
                .reaped
                .wait_timeout(state, remaining)
                .expect("queue state lock poisoned");
            state = next;
            if timeout.timed_out() {
                break;
            }
        }
        TaskQueueShutdownReport {
            active_count,
            forced,
        }
    }

    /// Stop the queue without waiting beyond one caller-owned shutdown deadline.
    ///
    /// The default [`Self::shutdown`] contract intentionally retains its two
    /// independent ten-second windows. Hosted parent-loss shutdown uses this
    /// stricter variant so task termination and active-record reaping consume
    /// one shared budget instead.
    pub fn shutdown_until(&self, deadline: Instant) -> TaskQueueShutdownReport {
        let (active_count, snapshot): (usize, Vec<ShutdownSnapshot>) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            state.shutdown = true;
            let active = state
                .active
                .iter()
                .map(|(reference, active)| (reference.clone(), Arc::clone(&active.process)))
                .collect::<Vec<_>>();
            let active_count = active.len();
            (active_count, active)
        };
        let references = snapshot
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        let (completed_send, completed_receive) = std::sync::mpsc::channel();
        let mut forced = false;
        for (_, process) in snapshot {
            if Instant::now() >= deadline {
                forced = true;
                break;
            }
            let completed_send = completed_send.clone();
            if thread::Builder::new()
                .spawn(move || {
                    let mut process = process.lock().expect("managed process lock poisoned");
                    let forced = process.terminate_exact_until(deadline).is_err()
                        || !process.cleanup_until(deadline);
                    if forced {
                        process.detach_after_bounded_shutdown();
                    }
                    let _ = completed_send.send(forced);
                })
                .is_err()
            {
                forced = true;
            }
        }
        drop(completed_send);

        for _ in 0..active_count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                forced = true;
                break;
            }
            match completed_receive.recv_timeout(remaining) {
                Ok(worker_forced) => forced |= worker_forced,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    forced = true;
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    forced = true;
                    break;
                }
            }
        }

        let mut state = self.inner.state.lock().expect("queue state lock poisoned");
        while references
            .iter()
            .any(|reference| state.active.contains_key(reference))
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                forced = true;
                break;
            }
            let (next, timeout) = self
                .inner
                .reaped
                .wait_timeout(state, remaining)
                .expect("queue state lock poisoned");
            state = next;
            if timeout.timed_out() {
                forced = true;
                break;
            }
        }
        TaskQueueShutdownReport {
            active_count,
            forced,
        }
    }

    pub fn collect_status_snapshot(&self, now: Instant) -> TaskQueueStatusSnapshot {
        let state = self.inner.state.lock().expect("queue state lock poisoned");
        let tasks = state
            .active
            .iter()
            .map(|(reference, active)| {
                let duration_seconds = now.saturating_duration_since(active.started_at).as_secs();
                let cap_seconds = active.cap.as_secs();
                let (slow, stuck) = task_status_flags(duration_seconds, cap_seconds);
                TaskStatus {
                    partition: active.partition.clone(),
                    reference: reference.clone(),
                    command: active.command.clone(),
                    duration_seconds,
                    cap_seconds,
                    slow,
                    stuck,
                }
            })
            .collect();
        let recent_tasks = state.history.iter().cloned().collect();
        let mut queues = state
            .queues
            .iter()
            .filter(|(_, queue)| !queue.is_empty())
            .map(|(partition, queue)| (partition.as_str().to_owned(), queue.len()))
            .collect::<BTreeMap<_, _>>();
        if !state.pending.is_empty() {
            queues.insert("pending".to_owned(), state.pending.len());
        }
        TaskQueueStatusSnapshot {
            tasks,
            recent_tasks,
            queues,
        }
    }

    pub fn collect_task_status(&self, now: Instant) -> Vec<TaskStatus> {
        self.collect_status_snapshot(now).tasks
    }

    pub fn collect_queue_counts(&self) -> BTreeMap<String, usize> {
        self.collect_status_snapshot(Instant::now()).queues
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

    /// Return the bounded, read-only completion projection for status consumers.
    pub fn history(&self) -> Vec<TaskHistoryRecord> {
        self.collect_status_snapshot(Instant::now()).recent_tasks
    }

    #[cfg(test)]
    fn set_worker_spawner(&self, spawner: QueueProcessSpawner) {
        *self
            .inner
            .worker_spawner
            .lock()
            .expect("queue worker spawner lock poisoned") = spawner;
    }

    #[cfg(test)]
    fn set_worker_thread_spawner(&self, spawner: WorkerThreadSpawner) {
        *self
            .inner
            .worker_thread_spawner
            .lock()
            .expect("queue worker-thread spawner lock poisoned") = spawner;
    }

    #[cfg(test)]
    fn set_catchup_admission_capability(&self, capability: CatchupAdmissionCapability) {
        *self
            .inner
            .catchup_admission_capability
            .lock()
            .expect("queue catchup-admission capability lock poisoned") = capability;
    }

    #[cfg(test)]
    fn join_test_workers(&self, expected: usize, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut handles = self
            .inner
            .worker_threads
            .lock()
            .expect("queue worker registry lock poisoned");
        while handles.len() < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "timed out waiting for {expected} queue workers; observed {}",
                    handles.len()
                ));
            }
            let (next, wait) = self
                .inner
                .worker_threads_changed
                .wait_timeout(handles, remaining)
                .expect("queue worker registry lock poisoned");
            handles = next;
            if wait.timed_out() && handles.len() < expected {
                return Err(format!(
                    "timed out waiting for {expected} queue workers; observed {}",
                    handles.len()
                ));
            }
        }
        let worker_handles = handles.drain(..expected).collect::<Vec<_>>();
        drop(handles);
        for handle in worker_handles {
            handle
                .join()
                .map_err(|_| "queue worker panicked".to_owned())?;
        }
        Ok(())
    }
}

fn task_status_flags(duration_seconds: u64, cap_seconds: u64) -> (bool, bool) {
    (
        (duration_seconds as u128) * 4 >= (cap_seconds as u128) * 3,
        duration_seconds > cap_seconds,
    )
}

// Normalize at this one consumer instead of widening request.rs with a trait used nowhere else.
fn normalize_request(request: ExecutionRequest, caps: &dyn CapResolver) -> Submission {
    match request {
        ExecutionRequest::Bus(request) => Submission {
            cap: caps.cap_for(&request.cmd.partition()),
            partition: request.cmd.partition(),
            command: request.cmd.as_wire().to_vec(),
            reference: request.reference,
            day: request.day,
            scheduler_name: request.scheduler_name,
            daily_catchup_provenance: request.daily_catchup_provenance,
        },
        ExecutionRequest::Scheduled(request) => Submission {
            cap: request
                .max_runtime
                .filter(|cap| !cap.is_zero())
                .unwrap_or_else(|| caps.cap_for(&request.cmd.partition())),
            partition: request.cmd.partition(),
            command: request.cmd.as_wire().to_vec(),
            reference: request.reference,
            day: request.day,
            scheduler_name: Some(request.scheduler_name),
            daily_catchup_provenance: None,
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
            .find(|entry| entry.command == submission.command && entry.cap == submission.cap)
        {
            if entry.references.contains(&submission.reference) {
                return (SubmitOutcome::DuplicateQueuedReference, None);
            }
            entry.references.push(submission.reference);
            return (SubmitOutcome::Coalesced, None);
        }
        // Coalesced callers add only refs: the first queued submitter owns its
        // day, scheduler, and automatic-catchup provenance.
        queue.push_back(QueuedEntry {
            cap: submission.cap,
            references: vec![submission.reference.clone()],
            command: submission.command.clone(),
            day: submission.day.clone(),
            scheduler_name: submission.scheduler_name.clone(),
            daily_catchup_provenance: submission.daily_catchup_provenance.clone(),
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
            daily_catchup_admission: None,
        }),
    )
}

fn start_dispatch(inner: Arc<QueueInner>, mut dispatch: Dispatch) {
    let partition = dispatch.submission.partition.clone();
    let reference = dispatch.submission.reference.clone();
    let worker_inner = Arc::clone(&inner);
    if let Some(provenance) = &dispatch.submission.daily_catchup_provenance {
        let started_at = unix_seconds_f64();
        #[cfg(test)]
        let admission = {
            let capability = Arc::clone(
                &inner
                    .catchup_admission_capability
                    .lock()
                    .expect("queue catchup-admission capability lock poisoned"),
            );
            admit_daily_catchup_with_capability(
                &inner.options.journal_root,
                &provenance.day,
                &dispatch.submission.reference,
                started_at,
                move || capability(),
            )
        };
        #[cfg(not(test))]
        let admission = admit_daily_catchup(
            &inner.options.journal_root,
            &provenance.day,
            &dispatch.submission.reference,
            started_at,
        );
        match admission {
            Ok(admission) => dispatch.daily_catchup_admission = Some(admission),
            Err(CatchupError::CapabilityUnavailable) => {
                record_completion(&inner, &dispatch, -1, "capability_unavailable".to_owned());
                let next = finish_worker(
                    &inner,
                    &dispatch.submission.partition,
                    &dispatch.submission.reference,
                );
                if let Some(next) = next {
                    start_dispatch(inner, next);
                }
                return;
            }
            Err(_) => {
                record_daily_catchup_admission_failure(
                    &inner.options.journal_root,
                    &provenance.day,
                    started_at,
                );
                record_completion(&inner, &dispatch, -1, "error".to_owned());
                let next = finish_worker(
                    &inner,
                    &dispatch.submission.partition,
                    &dispatch.submission.reference,
                );
                if let Some(next) = next {
                    start_dispatch(inner, next);
                }
                return;
            }
        }
    }
    let rollback = dispatch.clone();
    let worker = move || {
        let _lease = WorkerLease {
            inner: worker_inner.clone(),
            partition,
            reference,
        };
        run_worker(worker_inner, dispatch);
    };
    #[cfg(test)]
    let spawned = {
        let spawner = Arc::clone(
            &inner
                .worker_thread_spawner
                .lock()
                .expect("queue worker-thread spawner lock poisoned"),
        );
        spawner(Box::new(worker))
    };
    #[cfg(not(test))]
    let spawned = thread::Builder::new().spawn(worker);
    match spawned {
        Ok(handle) => {
            #[cfg(test)]
            {
                inner
                    .worker_threads
                    .lock()
                    .expect("queue worker registry lock poisoned")
                    .push(handle);
                inner.worker_threads_changed.notify_all();
            }
            #[cfg(not(test))]
            drop(handle);
        }
        Err(_) => {
            record_completion(&inner, &rollback, -1, "error".to_owned());
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
}

fn run_worker(inner: Arc<QueueInner>, dispatch: Dispatch) {
    let primary = dispatch.submission.reference.clone();
    let spawner = Arc::clone(
        &inner
            .worker_spawner
            .lock()
            .expect("queue worker spawner lock poisoned"),
    );
    let timeout = dispatch.submission.cap;
    let process = spawner(
        dispatch.submission.command.clone(),
        SpawnOptions {
            journal_root: inner.options.journal_root.clone(),
            reference: primary.clone(),
            day: dispatch.submission.day.clone(),
            sink: inner.options.process_sink.clone(),
            environment: inner.options.child_environment.clone(),
        },
        timeout,
    );
    let Ok(process) = process else {
        record_completion(&inner, &dispatch, -1, "error".to_owned());
        return;
    };
    let pid = process.lock().expect("managed process lock poisoned").pid();
    let started_at = Instant::now();
    let started_at_unix = unix_seconds();
    {
        let mut state = inner.state.lock().expect("queue state lock poisoned");
        state.active.insert(
            primary.clone(),
            ActiveEntry {
                cap: timeout,
                partition: dispatch.submission.partition.clone(),
                command: dispatch.submission.command.clone(),
                started_at,
                started_at_unix,
                pid,
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
            Err(_) => {
                let _ = process
                    .lock()
                    .expect("managed process lock poisoned")
                    .terminate_exact(CAP_TERMINATION_TIMEOUT);
                break -1;
            }
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
        inner.reaped.notify_all();
        status
    };
    if let (Some(provenance), Some(admission)) = (
        &dispatch.submission.daily_catchup_provenance,
        &dispatch.daily_catchup_admission,
    ) {
        let timed_out = status == TIMEOUT_EXIT_STATUS;
        if let Err(error) = record_daily_catchup_outcome(
            &inner.options.journal_root,
            &provenance.day,
            &dispatch.submission.reference,
            admission.generation,
            &admission.fingerprint,
            DailyCatchupOutcome {
                success: exit_code == 0 && !timed_out,
                timed_out,
                timeout_seconds: timed_out.then_some(dispatch.submission.cap.as_secs_f64()),
                ended_at: unix_seconds_f64(),
                exit_code,
                exit_status: status,
            },
        ) {
            eprintln!("failed to record daily catchup outcome: {error}");
        }
    }
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
                    cap: entry.cap,
                    partition: partition.clone(),
                    command: entry.command,
                    reference: entry.references[0].clone(),
                    day: entry.day,
                    scheduler_name: entry.scheduler_name,
                    daily_catchup_provenance: entry.daily_catchup_provenance,
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
                    daily_catchup_admission: None,
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
        // Sinks are best-effort; a caller bug must not interrupt queue state transitions.
        let _ = catch_unwind(AssertUnwindSafe(|| sink.emit(event)));
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
    process: QueueProcessHandle,
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
    process: QueueProcessHandle,
    timeout: Duration,
) {
    let _ = process
        .lock()
        .expect("managed process lock poisoned")
        .terminate_exact(timeout);
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

fn unix_seconds_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Barrier, Condvar, mpsc};

    use super::*;
    use crate::cap::{DEFAULT_TASK_MAX_RUNTIME, DefaultCapResolver};
    use crate::request::{BusTaskRequest, DailyCatchupProvenance, TaskArgv};

    struct FixedCap(u64);
    impl CapResolver for FixedCap {
        fn cap_for(&self, _partition: &Partition) -> Duration {
            Duration::from_secs(self.0)
        }
    }

    enum Poll {
        Error,
        Complete(i32),
        Gate {
            arrived: Arc<Barrier>,
            release: Arc<Barrier>,
            code: i32,
        },
    }

    struct FakeProcess {
        pid: u32,
        polls: VecDeque<Poll>,
        terminate_error: bool,
        cleanups: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl FakeProcess {
        fn idle(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::new(),
                terminate_error: false,
                cleanups,
            }
        }

        fn poll_error(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::from([Poll::Error]),
                terminate_error: true,
                cleanups,
            }
        }

        fn complete(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::from([Poll::Complete(0)]),
                terminate_error: false,
                cleanups,
            }
        }

        fn gated(
            arrived: Arc<Barrier>,
            release: Arc<Barrier>,
            cleanups: Arc<std::sync::atomic::AtomicUsize>,
        ) -> Self {
            Self {
                pid: 1,
                polls: VecDeque::from([Poll::Gate {
                    arrived,
                    release,
                    code: 0,
                }]),
                terminate_error: false,
                cleanups,
            }
        }
    }

    impl QueueProcess for FakeProcess {
        fn pid(&self) -> u32 {
            self.pid
        }

        fn poll(&mut self) -> io::Result<Option<i32>> {
            match self.polls.pop_front() {
                Some(Poll::Error) => Err(io::Error::other("poll failure")),
                Some(Poll::Complete(code)) => Ok(Some(code)),
                Some(Poll::Gate {
                    arrived,
                    release,
                    code,
                }) => {
                    arrived.wait();
                    release.wait();
                    Ok(Some(code))
                }
                None => Ok(None),
            }
        }

        fn terminate_exact(
            &mut self,
            _timeout: Duration,
        ) -> Result<TerminationOutcome, TerminationError> {
            if self.terminate_error {
                Err(TerminationError::ParentGraceTimeout)
            } else {
                Ok(TerminationOutcome::Graceful { exit_code: None })
            }
        }

        fn terminate_exact_until(
            &mut self,
            _deadline: Instant,
        ) -> Result<TerminationOutcome, TerminationError> {
            self.terminate_exact(Duration::ZERO)
        }

        fn cleanup(&mut self) {
            self.cleanups
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn cleanup_until(&mut self, _deadline: Instant) -> bool {
            self.cleanup();
            true
        }

        fn detach_after_bounded_shutdown(&mut self) {}
    }

    enum SpawnPlan {
        Failure,
        Process(FakeProcess),
    }

    fn plan_spawner(plans: VecDeque<SpawnPlan>) -> QueueProcessSpawner {
        let plans = Mutex::new(plans);
        Arc::new(
            move |_, _, _| match plans.lock().expect("fake plans").pop_front() {
                Some(SpawnPlan::Failure) => Err(SpawnError::EmptyCommand),
                Some(SpawnPlan::Process(process)) => Ok(Arc::new(Mutex::new(Box::new(process)))),
                None => panic!("missing fake process plan"),
            },
        )
    }

    fn gated_plan(
        arrived: Arc<Barrier>,
        release: Arc<Barrier>,
        cleanups: Arc<std::sync::atomic::AtomicUsize>,
    ) -> SpawnPlan {
        SpawnPlan::Process(FakeProcess::gated(arrived, release, cleanups))
    }

    fn queue(ready: bool, cap: u64, plans: VecDeque<SpawnPlan>) -> TaskQueue {
        queue_with_sink(ready, cap, plans, None)
    }

    struct UnreachableProcessStateProbe;

    impl ProcessStateProbe for UnreachableProcessStateProbe {
        fn state(&self, _pid: u32) -> ProcessState {
            panic!("routine queue unit tests must not reach the process-state probe");
        }
    }

    fn queue_with_sink(
        ready: bool,
        cap: u64,
        plans: VecDeque<SpawnPlan>,
        queue_sink: Option<Arc<dyn TaskQueueEventSink>>,
    ) -> TaskQueue {
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: PathBuf::new(),
            cap_resolver: Arc::new(FixedCap(cap)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink,
            process_sink: None,
            ready,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(plan_spawner(plans));
        queue
    }

    const TEST_TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum StatusSeam {
        SentinelStarted = 1,
        SentinelCompleted = 2,
        FollowerPopped = 3,
        FollowerStarted = 4,
        FollowerReleased = 5,
    }

    impl StatusSeam {
        fn rank(self) -> u8 {
            self as u8
        }
    }

    #[derive(Default)]
    struct SeamControl {
        released: u8,
        cancelled: bool,
    }

    struct StatusSeamSink {
        events: mpsc::Sender<StatusSeam>,
        control: StatusSeamControl,
    }

    type StatusSeamControl = Arc<(Mutex<SeamControl>, Condvar)>;
    type StatusSeamParts = (
        Arc<dyn TaskQueueEventSink>,
        mpsc::Receiver<StatusSeam>,
        StatusSeamControl,
    );

    impl TaskQueueEventSink for StatusSeamSink {
        fn emit(&self, event: TaskQueueEvent) {
            let seam = match event {
                TaskQueueEvent::Started { reference, .. } if reference == "sentinel" => {
                    Some(StatusSeam::SentinelStarted)
                }
                TaskQueueEvent::Stopped { reference, .. } if reference == "sentinel" => {
                    Some(StatusSeam::SentinelCompleted)
                }
                TaskQueueEvent::QueueChanged {
                    running_reference: Some(reference),
                    queued_depth: 0,
                    ..
                } if reference == "follower" => Some(StatusSeam::FollowerPopped),
                TaskQueueEvent::Started { reference, .. } if reference == "follower" => {
                    Some(StatusSeam::FollowerStarted)
                }
                TaskQueueEvent::QueueChanged {
                    running_reference: None,
                    queued_depth: 0,
                    ..
                } => Some(StatusSeam::FollowerReleased),
                _ => None,
            };
            let Some(seam) = seam else {
                return;
            };
            if self.events.send(seam).is_err() {
                return;
            }
            let (lock, changed) = &*self.control;
            let control = lock.lock().expect("status seam control poisoned");
            let (control, wait) = changed
                .wait_timeout_while(control, TEST_TRANSITION_TIMEOUT, |control| {
                    !control.cancelled && control.released < seam.rank()
                })
                .expect("status seam control poisoned");
            assert!(
                control.cancelled || control.released >= seam.rank() || !wait.timed_out(),
                "timed out waiting to release status seam {seam:?}"
            );
        }
    }

    struct StatusSeamHarness {
        queue: TaskQueue,
        events: mpsc::Receiver<StatusSeam>,
        control: StatusSeamControl,
        expected_workers: usize,
        finished: bool,
    }

    impl StatusSeamHarness {
        fn new(
            queue: TaskQueue,
            events: mpsc::Receiver<StatusSeam>,
            control: StatusSeamControl,
        ) -> Self {
            Self {
                queue,
                events,
                control,
                expected_workers: 0,
                finished: false,
            }
        }

        fn expect_workers(&mut self, expected: usize) {
            self.expected_workers = expected;
        }

        fn wait_for(&self, expected: StatusSeam) {
            let actual = self
                .events
                .recv_timeout(TEST_TRANSITION_TIMEOUT)
                .unwrap_or_else(|error| {
                    panic!("timed out waiting for status seam {expected:?}: {error}")
                });
            assert_eq!(actual, expected, "unexpected queue status seam");
        }

        fn release(&self, seam: StatusSeam) {
            let (lock, changed) = &*self.control;
            let mut control = lock.lock().expect("status seam control poisoned");
            control.released = control.released.max(seam.rank());
            changed.notify_all();
        }

        fn finish(mut self) {
            let result = self
                .queue
                .join_test_workers(self.expected_workers, TEST_TRANSITION_TIMEOUT);
            self.finished = true;
            result.unwrap_or_else(|error| panic!("failed to join queue workers: {error}"));
        }
    }

    impl Drop for StatusSeamHarness {
        fn drop(&mut self) {
            if self.finished {
                return;
            }
            let (lock, changed) = &*self.control;
            lock.lock().expect("status seam control poisoned").cancelled = true;
            changed.notify_all();
            let _ = self
                .queue
                .join_test_workers(self.expected_workers, TEST_TRANSITION_TIMEOUT);
        }
    }

    fn status_seam_sink() -> StatusSeamParts {
        let (events, receiver) = mpsc::channel();
        let control = Arc::new((Mutex::new(SeamControl::default()), Condvar::new()));
        (
            Arc::new(StatusSeamSink {
                events,
                control: Arc::clone(&control),
            }),
            receiver,
            control,
        )
    }

    fn dispatch(reference: &str) -> Dispatch {
        Dispatch {
            submission: Submission {
                cap: Duration::from_secs(10),
                partition: Partition::new("svc"),
                command: vec!["svc".to_owned()],
                reference: reference.to_owned(),
                day: None,
                scheduler_name: None,
                daily_catchup_provenance: None,
            },
            references: vec![reference.to_owned()],
            daily_catchup_admission: None,
        }
    }

    fn add_active(queue: &TaskQueue, reference: &str) {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let process: QueueProcessHandle =
            Arc::new(Mutex::new(Box::new(FakeProcess::idle(1, cleanups))));
        add_active_process(queue, reference, process);
    }

    fn add_active_process(queue: &TaskQueue, reference: &str, process: QueueProcessHandle) {
        queue
            .inner
            .state
            .lock()
            .expect("queue state")
            .active
            .insert(
                reference.to_owned(),
                ActiveEntry {
                    cap: queue
                        .inner
                        .options
                        .cap_resolver
                        .cap_for(&Partition::new("svc")),
                    partition: Partition::new("svc"),
                    command: vec!["svc".to_owned()],
                    started_at: Instant::now(),
                    started_at_unix: 0,
                    pid: 1,
                    process,
                },
            );
    }

    #[test]
    fn shutdown_reports_forced_worker_termination() {
        let queue = queue(true, 10, VecDeque::new());
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut process = FakeProcess::idle(1, cleanups);
        process.terminate_error = true;
        add_active_process(
            &queue,
            "escalating",
            Arc::new(Mutex::new(Box::new(process))),
        );

        let inner = Arc::clone(&queue.inner);
        let reaper = thread::spawn(move || {
            loop {
                let mut state = inner.state.lock().expect("queue state lock poisoned");
                if state.shutdown {
                    state.active.remove("escalating");
                    inner.reaped.notify_all();
                    return;
                }
                drop(state);
                thread::sleep(Duration::from_millis(1));
            }
        });

        let report = queue.shutdown();
        reaper.join().expect("test reaper");
        assert_eq!(report.active_count, 1);
        assert!(report.forced);
    }

    fn request(reference: &str) -> ExecutionRequest {
        ExecutionRequest::Bus(BusTaskRequest {
            cmd: TaskArgv::from_wire(vec!["svc".to_owned()]).expect("command"),
            reference: reference.to_owned(),
            day: None,
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            daily_catchup_provenance: None,
        })
    }

    fn command_request(
        command: &[&str],
        reference: &str,
        provenance: Option<DailyCatchupProvenance>,
    ) -> ExecutionRequest {
        ExecutionRequest::Bus(BusTaskRequest {
            cmd: TaskArgv::from_wire(command.iter().map(|value| (*value).to_owned()).collect())
                .expect("command"),
            reference: reference.to_owned(),
            day: provenance.as_ref().map(|value| value.day.clone()),
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            daily_catchup_provenance: provenance,
        })
    }

    #[derive(Default)]
    struct RecordingEventSink(Mutex<Vec<TaskQueueEvent>>);

    impl TaskQueueEventSink for RecordingEventSink {
        fn emit(&self, event: TaskQueueEvent) {
            self.0.lock().expect("recording sink").push(event);
        }
    }

    #[test]
    fn status_snapshot_is_ordered_coherent_and_stable() {
        let queue = queue(true, 10, VecDeque::new());
        add_active(&queue, "z");
        add_active(&queue, "a");
        record_completion(&queue.inner, &dispatch("old"), 0, "ok".to_owned());
        queue
            .inner
            .state
            .lock()
            .expect("queue state")
            .queues
            .insert(
                Partition::new("svc"),
                VecDeque::from([QueuedEntry {
                    cap: Duration::from_secs(10),
                    references: vec!["queued".to_owned()],
                    command: vec!["svc".to_owned()],
                    day: None,
                    scheduler_name: None,
                    daily_catchup_provenance: None,
                }]),
            );
        let now = Instant::now();
        let first = queue.collect_status_snapshot(now);
        assert_eq!(
            first
                .tasks
                .iter()
                .map(|task| task.reference.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(first.recent_tasks[0].reference, "old");
        assert_eq!(first.queues.get("svc"), Some(&1));
        assert_eq!(first, queue.collect_status_snapshot(now));
    }

    #[test]
    fn status_flags_keep_thresholds_without_saturation() {
        assert_eq!(task_status_flags(2, 4), (false, false));
        assert_eq!(task_status_flags(3, 4), (true, false));
        assert_eq!(task_status_flags(4, 4), (true, false));
        assert_eq!(task_status_flags(5, 4), (true, true));
        assert_eq!(task_status_flags(0, 0), (true, false));
        assert_eq!(task_status_flags(1, 0), (true, true));
        let cap = u64::MAX;
        let below = 13_835_058_055_282_163_711;
        let oracle = cap - cap / 4;
        assert_eq!(below < oracle, !task_status_flags(below, cap).0);
        assert_eq!(below + 1 >= oracle, task_status_flags(below + 1, cap).0);
    }

    #[test]
    fn snapshot_history_is_fifo_and_keeps_active_reference() {
        let queue = queue(true, 10, VecDeque::new());
        for index in 0..101 {
            record_completion(
                &queue.inner,
                &dispatch(&format!("ref-{index}")),
                0,
                "ok".to_owned(),
            );
        }
        add_active(&queue, "ref-1");
        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snapshot.recent_tasks.len(), HISTORY_LIMIT);
        for (record, reference) in [
            (&snapshot.recent_tasks[0], "ref-1"),
            (&snapshot.recent_tasks[99], "ref-100"),
        ] {
            assert_eq!(record.reference, reference);
            assert_eq!(record.partition, Partition::new("svc"));
            assert_eq!(record.command, ["svc"]);
            assert!(record.ended_at.duration_since(UNIX_EPOCH).is_ok());
            assert_eq!(record.exit_status, "ok");
            assert_eq!(record.scheduler_name, None);
        }
        assert_eq!(snapshot.tasks[0].reference, "ref-1");
        assert!(
            snapshot
                .recent_tasks
                .iter()
                .any(|record| record.reference == "ref-1")
        );
    }

    #[test]
    fn legacy_projections_preserve_queue_count_rules() {
        let pending = queue(false, 10, VecDeque::new());
        assert_eq!(
            pending.collect_status_snapshot(Instant::now()),
            TaskQueueStatusSnapshot {
                tasks: Vec::new(),
                recent_tasks: Vec::new(),
                queues: BTreeMap::new(),
            }
        );
        pending.submit(request("pending"));
        assert_eq!(pending.collect_queue_counts().get("pending"), Some(&1));
        let queue = queue(true, 10, VecDeque::new());
        queue
            .inner
            .state
            .lock()
            .expect("queue state")
            .running
            .insert(
                Partition::new("svc"),
                RunningSlot {
                    reference: "running".to_owned(),
                },
            );
        queue.submit(request("one"));
        queue.submit(request("two"));
        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snapshot.queues.get("svc"), Some(&1));
        assert_eq!(queue.collect_queue_counts(), snapshot.queues);
        assert_eq!(queue.history(), snapshot.recent_tasks);
        assert_eq!(queue.collect_task_status(Instant::now()), snapshot.tasks);
    }

    #[test]
    fn status_snapshot_covers_every_normal_completion_seam_and_rejects_torn_reads() {
        let (sink, events, control) = status_seam_sink();
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = queue_with_sink(
            true,
            10,
            VecDeque::from([
                SpawnPlan::Process(FakeProcess::complete(1, Arc::clone(&cleanups))),
                SpawnPlan::Process(FakeProcess::complete(2, cleanups)),
            ]),
            Some(sink),
        );
        let mut harness = StatusSeamHarness::new(queue.clone(), events, control);
        harness.expect_workers(1);

        queue.submit(request("sentinel"));
        harness.wait_for(StatusSeam::SentinelStarted);
        queue.submit(request("follower"));
        harness.expect_workers(2);
        let active = queue.collect_status_snapshot(Instant::now());
        assert_eq!(
            active
                .tasks
                .iter()
                .map(|task| task.reference.as_str())
                .collect::<Vec<_>>(),
            ["sentinel"]
        );
        assert!(active.recent_tasks.is_empty());
        assert_eq!(active.queues.get("svc"), Some(&1));
        harness.release(StatusSeam::SentinelStarted);

        harness.wait_for(StatusSeam::SentinelCompleted);
        let completed = queue.collect_status_snapshot(Instant::now());
        assert!(completed.tasks.is_empty());
        assert_eq!(completed.recent_tasks[0].reference, "sentinel");
        assert_eq!(completed.queues.get("svc"), Some(&1));
        harness.release(StatusSeam::SentinelCompleted);

        harness.wait_for(StatusSeam::FollowerPopped);
        let popped = queue.collect_status_snapshot(Instant::now());
        assert!(popped.tasks.is_empty());
        assert_eq!(popped.recent_tasks[0].reference, "sentinel");
        assert!(popped.queues.is_empty());

        let legacy_torn = TaskQueueStatusSnapshot {
            tasks: active.tasks.clone(),
            recent_tasks: popped.recent_tasks.clone(),
            queues: popped.queues.clone(),
        };
        assert_eq!(legacy_torn.tasks[0].reference, "sentinel");
        assert_eq!(legacy_torn.recent_tasks[0].reference, "sentinel");
        assert!(legacy_torn.queues.is_empty());
        assert_ne!(legacy_torn, active);
        assert_ne!(legacy_torn, completed);
        assert_ne!(legacy_torn, popped);
        harness.release(StatusSeam::FollowerPopped);

        harness.wait_for(StatusSeam::FollowerStarted);
        let follower = queue.collect_status_snapshot(Instant::now());
        assert_eq!(follower.tasks[0].reference, "follower");
        assert_eq!(follower.recent_tasks[0].reference, "sentinel");
        assert!(follower.queues.is_empty());
        harness.release(StatusSeam::FollowerStarted);

        harness.wait_for(StatusSeam::FollowerReleased);
        let idle = queue.collect_status_snapshot(Instant::now());
        assert!(idle.tasks.is_empty());
        assert_eq!(
            idle.recent_tasks
                .iter()
                .map(|task| task.reference.as_str())
                .collect::<Vec<_>>(),
            ["sentinel", "follower"]
        );
        assert!(idle.queues.is_empty());
        harness.release(StatusSeam::FollowerReleased);
        harness.finish();
    }

    #[test]
    fn spawn_failure_completes_and_advances_the_follower() {
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = queue(
            true,
            10,
            VecDeque::from([
                SpawnPlan::Failure,
                gated_plan(Arc::clone(&arrived), Arc::clone(&release), cleanups),
            ]),
        );
        queue.submit(request("failed"));
        queue.submit(request("follower"));
        arrived.wait();
        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snapshot.recent_tasks[0].reference, "failed");
        assert_eq!(snapshot.recent_tasks[0].exit_status, "error");
        assert_eq!(snapshot.tasks[0].reference, "follower");
        assert!(snapshot.queues.is_empty());
        release.wait();
    }

    #[cfg(unix)]
    #[test]
    fn catchup_child_spawn_failure_records_one_primary_terminal_outcome() {
        let journal = tempfile::tempdir().expect("journal");
        let day = "20260101";
        let health = journal.path().join("chronicle").join(day).join("health");
        fs::create_dir_all(&health).expect("health");
        fs::write(
            health.join("stream.updated"),
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        )
        .expect("stream marker");
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(plan_spawner(VecDeque::from([SpawnPlan::Failure])));
        let provenance = DailyCatchupProvenance {
            day: day.to_owned(),
        };

        assert_eq!(
            queue.submit(command_request(
                &["svc", "catchup"],
                "catchup",
                Some(provenance),
            )),
            SubmitOutcome::Dispatched,
        );
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("catchup worker");

        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(crate::catchup::catchup_state_path(journal.path())).expect("catchup state"),
        )
        .expect("catchup JSON");
        let record = &state["entries"]
            [crate::catchup::catchup_state_key(day, crate::catchup::KIND_DAILY_CATCHUP)];
        assert_eq!(record["attempts"], 1);
        assert_eq!(record["active"], serde_json::Value::Null);
        assert_eq!(record["last_outcome"], "error");
        assert!(record["next_retry_at"].as_f64().unwrap() > 0.0);
    }

    #[cfg(unix)]
    #[test]
    fn queued_catchup_samples_primary_admission_and_retains_it_for_terminal_correlation() {
        let journal = tempfile::tempdir().expect("journal");
        let day = "20260101";
        let health = journal.path().join("chronicle").join(day).join("health");
        fs::create_dir_all(&health).expect("health");
        assert_eq!(
            solstone_core_journal_io::bump_stream_marker(journal.path(), day)
                .expect("initial generation"),
            1
        );
        let first_arrived = Arc::new(Barrier::new(2));
        let first_release = Arc::new(Barrier::new(2));
        let catchup_arrived = Arc::new(Barrier::new(2));
        let catchup_release = Arc::new(Barrier::new(2));
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(plan_spawner(VecDeque::from([
            gated_plan(
                Arc::clone(&first_arrived),
                Arc::clone(&first_release),
                Arc::clone(&cleanups),
            ),
            gated_plan(
                Arc::clone(&catchup_arrived),
                Arc::clone(&catchup_release),
                cleanups,
            ),
        ])));
        assert_eq!(
            queue.submit(command_request(&["svc", "blocker"], "blocker", None)),
            SubmitOutcome::Dispatched,
        );
        first_arrived.wait();
        assert_eq!(
            queue.submit(command_request(
                &["svc", "catchup"],
                "catchup",
                Some(DailyCatchupProvenance {
                    day: day.to_owned(),
                }),
            )),
            SubmitOutcome::Queued,
        );

        let segment = journal.path().join("chronicle").join(day).join("120000_60");
        fs::create_dir_all(&segment).expect("segment");
        fs::write(segment.join("chat.jsonl"), b"new while queued\n").expect("raw mutation");
        assert_eq!(
            solstone_core_journal_io::bump_stream_marker(journal.path(), day)
                .expect("queued mutation generation"),
            2
        );
        let admitted_fingerprint =
            crate::catchup::read_raw_input_fingerprint(journal.path(), day).expect("fingerprint");
        first_release.wait();
        catchup_arrived.wait();

        let active: serde_json::Value = serde_json::from_slice(
            &fs::read(crate::catchup::catchup_state_path(journal.path())).expect("catchup state"),
        )
        .expect("catchup JSON");
        let active = &active["entries"]
            [crate::catchup::catchup_state_key(day, crate::catchup::KIND_DAILY_CATCHUP)];
        assert_eq!(active["admitted_generation"], 2);
        assert_eq!(active["fingerprint"], admitted_fingerprint);
        assert_eq!(active["active"]["ref"], "catchup");

        fs::write(
            health.join("daily.updated"),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "generation": 2,
                "fingerprint": admitted_fingerprint,
            }))
            .expect("daily marker"),
        )
        .expect("publish admitted generation");
        assert_eq!(
            solstone_core_journal_io::bump_stream_marker(journal.path(), day)
                .expect("later dirty generation"),
            3
        );
        catchup_release.wait();
        queue
            .join_test_workers(2, TEST_TRANSITION_TIMEOUT)
            .expect("queue workers");

        let terminal: serde_json::Value = serde_json::from_slice(
            &fs::read(crate::catchup::catchup_state_path(journal.path())).expect("catchup state"),
        )
        .expect("catchup JSON");
        let terminal = &terminal["entries"]
            [crate::catchup::catchup_state_key(day, crate::catchup::KIND_DAILY_CATCHUP)];
        assert_eq!(terminal["admitted_generation"], 2);
        assert_eq!(terminal["last_outcome"], "completed");
        assert_eq!(terminal["active"], serde_json::Value::Null);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_primary_admission_records_terminal_backoff_without_spawning_child() {
        let journal = tempfile::tempdir().expect("journal");
        let day = "20260101";
        let health = journal.path().join("chronicle").join(day).join("health");
        fs::create_dir_all(&health).expect("health");
        fs::write(health.join("stream.updated"), b"malformed").expect("malformed marker");
        let child_spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spawn_count = Arc::clone(&child_spawns);
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(Arc::new(move |_, _, _| {
            spawn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(SpawnError::EmptyCommand)
        }));

        assert_eq!(
            queue.submit(command_request(
                &["svc", "catchup"],
                "catchup",
                Some(DailyCatchupProvenance {
                    day: day.to_owned(),
                }),
            )),
            SubmitOutcome::Dispatched,
        );
        assert_eq!(
            child_spawns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "unreadable admission must fail before child spawn"
        );
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(crate::catchup::catchup_state_path(journal.path())).expect("catchup state"),
        )
        .expect("catchup JSON");
        let record = &state["entries"]
            [crate::catchup::catchup_state_key(day, crate::catchup::KIND_DAILY_CATCHUP)];
        assert_eq!(record["active"], serde_json::Value::Null);
        assert_eq!(record["last_outcome"], "error");
        assert_eq!(record["reason_code"], "admission_unreadable");
        assert!(record["next_retry_at"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn capability_unavailable_primary_admission_keeps_the_ledger_untouched_without_spawning() {
        let journal = tempfile::tempdir().expect("journal");
        let day = "20260101";
        let state_path = crate::catchup::catchup_state_path(journal.path());
        fs::create_dir_all(state_path.parent().expect("health directory")).expect("health");
        fs::write(
            &state_path,
            br#"{"version":1,"entries":{"20260101:daily-catchup":{"sentinel":"keep"}}}"#,
        )
        .expect("seed catchup state");
        let before = fs::read(&state_path).expect("seeded catchup state");
        let child_spawns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let spawn_count = Arc::clone(&child_spawns);
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(Arc::new(move |_, _, _| {
            spawn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(SpawnError::EmptyCommand)
        }));
        queue.set_catchup_admission_capability(Arc::new(|| {
            Err(CatchupError::CapabilityUnavailable)
        }));
        assert_eq!(
            queue.submit(command_request(
                &["svc", "catchup"],
                "catchup",
                Some(DailyCatchupProvenance {
                    day: day.to_owned(),
                }),
            )),
            SubmitOutcome::Dispatched,
        );

        assert_eq!(
            child_spawns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "capability refusal must happen before worker spawn"
        );
        assert_eq!(fs::read(&state_path).expect("catchup state"), before);
        assert_eq!(queue.history()[0].exit_status, "capability_unavailable");
    }

    #[cfg(unix)]
    #[test]
    fn catchup_worker_thread_spawn_failure_records_terminal_outcome() {
        let journal = tempfile::tempdir().expect("journal");
        let day = "20260101";
        let health = journal.path().join("chronicle").join(day).join("health");
        fs::create_dir_all(&health).expect("health");
        fs::write(
            health.join("stream.updated"),
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        )
        .expect("stream marker");
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_thread_spawner(Arc::new(|_| {
            Err(io::Error::other("injected worker-thread spawn failure"))
        }));
        let provenance = DailyCatchupProvenance {
            day: day.to_owned(),
        };

        assert_eq!(
            queue.submit(command_request(
                &["svc", "catchup"],
                "catchup",
                Some(provenance),
            )),
            SubmitOutcome::Dispatched,
        );

        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(crate::catchup::catchup_state_path(journal.path())).expect("catchup state"),
        )
        .expect("catchup JSON");
        let record = &state["entries"]
            [crate::catchup::catchup_state_key(day, crate::catchup::KIND_DAILY_CATCHUP)];
        assert_eq!(record["attempts"], 1);
        assert_eq!(record["active"], serde_json::Value::Null);
        assert_eq!(record["last_outcome"], "error");
    }

    #[test]
    fn coalesced_follower_stops_without_owning_catchup_lifecycle() {
        let journal = tempfile::tempdir().expect("journal");
        let day = "20260101";
        let health = journal.path().join("chronicle").join(day).join("health");
        fs::create_dir_all(&health).expect("health");
        fs::write(
            health.join("stream.updated"),
            br#"{"version":1,"generation":1,"fingerprint":null}"#,
        )
        .expect("stream marker");
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink = Arc::new(RecordingEventSink::default());
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: Some(Arc::clone(&sink) as Arc<dyn TaskQueueEventSink>),
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(plan_spawner(VecDeque::from([
            gated_plan(
                Arc::clone(&arrived),
                Arc::clone(&release),
                Arc::clone(&cleanups),
            ),
            SpawnPlan::Process(FakeProcess::complete(2, cleanups)),
        ])));
        assert_eq!(
            queue.submit(command_request(&["svc", "blocker"], "blocker", None)),
            SubmitOutcome::Dispatched,
        );
        arrived.wait();
        let provenance = DailyCatchupProvenance {
            day: day.to_owned(),
        };
        assert_eq!(
            queue.submit(command_request(
                &["svc", "catchup"],
                "primary",
                Some(provenance),
            )),
            SubmitOutcome::Queued,
        );
        assert_eq!(
            queue.submit(command_request(&["svc", "catchup"], "follower", None,)),
            SubmitOutcome::Coalesced,
        );
        release.wait();
        queue
            .join_test_workers(2, TEST_TRANSITION_TIMEOUT)
            .expect("queue workers");

        let events = sink.0.lock().expect("recorded events");
        let started = events
            .iter()
            .filter_map(|event| match event {
                TaskQueueEvent::Started { reference, .. } => Some(reference.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let stopped = events
            .iter()
            .filter_map(|event| match event {
                TaskQueueEvent::Stopped { reference, .. } => Some(reference.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(started.contains(&"primary"));
        assert!(!started.contains(&"follower"));
        assert!(stopped.contains(&"primary"));
        assert!(stopped.contains(&"follower"));

        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(crate::catchup::catchup_state_path(journal.path())).expect("catchup state"),
        )
        .expect("catchup JSON");
        let record = &state["entries"]
            [crate::catchup::catchup_state_key(day, crate::catchup::KIND_DAILY_CATCHUP)];
        assert_eq!(record["attempts"], 1);
        assert_eq!(record["active"], serde_json::Value::Null);
    }

    #[test]
    fn poll_and_terminate_errors_still_cleanup_and_advance() {
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = queue(
            true,
            10,
            VecDeque::from([
                SpawnPlan::Process(FakeProcess::poll_error(1, Arc::clone(&cleanups))),
                gated_plan(
                    Arc::clone(&arrived),
                    Arc::clone(&release),
                    Arc::clone(&cleanups),
                ),
            ]),
        );
        queue.submit(request("failed"));
        queue.submit(request("follower"));
        arrived.wait();
        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert_eq!(cleanups.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(snapshot.recent_tasks[0].exit_status, "error");
        assert_eq!(snapshot.tasks[0].reference, "follower");
        assert!(snapshot.queues.is_empty());
        release.wait();
    }

    #[test]
    fn scheduled_budgets_survive_pending_queueing_and_do_not_coalesce_different_caps() {
        use crate::request::{ScheduledArgv, ScheduledRequest};
        let queue = queue(false, 42, VecDeque::new());
        let captured = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&captured);
        queue.set_worker_spawner(Arc::new(move |_, options, timeout| {
            recorded
                .lock()
                .expect("captured budgets")
                .push((options.reference, timeout));
            Ok(Arc::new(Mutex::new(Box::new(FakeProcess::complete(
                1,
                Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            )))))
        }));
        for (reference, cap) in [("short", 3), ("long", 7), ("zero", 0)] {
            let mut scheduled = ScheduledRequest::new(
                ScheduledArgv::from_wire(vec!["svc".to_owned()]).expect("argv"),
                reference,
                "scheduled",
            );
            scheduled.max_runtime = Some(Duration::from_secs(cap));
            assert_eq!(
                queue.submit(ExecutionRequest::Scheduled(scheduled)),
                SubmitOutcome::Pending
            );
        }
        assert_eq!(queue.submit(request("bus")), SubmitOutcome::Pending);
        queue.set_ready();
        // The zero override and ordinary bus request share the same effective
        // budget and may coalesce; different scheduled budgets must not.
        queue
            .join_test_workers(3, TEST_TRANSITION_TIMEOUT)
            .expect("workers");
        let mut actual = captured.lock().expect("budgets").clone();
        actual.sort();
        assert_eq!(
            actual,
            vec![
                ("long".to_owned(), Duration::from_secs(7)),
                ("short".to_owned(), Duration::from_secs(3)),
                ("zero".to_owned(), Duration::from_secs(42)),
            ]
        );
    }

    #[test]
    fn worker_spawner_receives_the_resolver_cap_for_the_dispatched_partition() {
        let partition = Partition::new("svc");
        let override_cap = Duration::from_secs(42);
        assert_ne!(override_cap, DEFAULT_TASK_MAX_RUNTIME);
        let mut resolver = DefaultCapResolver::default();
        resolver.set_override(partition.clone(), override_cap);
        let resolver = Arc::new(resolver);
        let captured = Arc::new(Mutex::new(None));
        let recorded = Arc::clone(&captured);
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue = TaskQueue::new(TaskQueueOptions {
            journal_root: PathBuf::new(),
            cap_resolver: Arc::clone(&resolver) as Arc<dyn CapResolver + Send + Sync>,
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
        });
        queue.set_worker_spawner(Arc::new(move |_, _, timeout| {
            *recorded.lock().expect("captured timeout") = Some(timeout);
            Ok(Arc::new(Mutex::new(Box::new(FakeProcess::complete(
                1,
                Arc::clone(&cleanups),
            )))))
        }));
        queue.submit(request("cap-check"));
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("queue worker");
        assert_eq!(
            *captured.lock().expect("captured timeout"),
            Some(resolver.cap_for(&partition))
        );
    }
}
