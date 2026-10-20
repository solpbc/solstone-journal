// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-partition task admission, lifecycle, and deadline enforcement.

use crate::priority::PriorityIntent;
use crate::work_policy::{ThinkingLane, WorkPolicy};
use solstone_core_journal_config::work_priority::{WORK_PRIORITY_ENV, WorkPriority};
use solstone_core_local::admission::confidential_background_open;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cap::CapResolver;
#[cfg(not(test))]
use crate::catchup::admit_daily_catchup;
use crate::catchup::{
    CatchupError, DailyCatchupAdmission, DailyCatchupOutcome,
    record_daily_catchup_admission_failure, record_daily_catchup_outcome_with_roots,
};
#[cfg(test)]
use crate::catchup::{admit_daily_catchup_with_capability, catchup_marker_capability};
use crate::partition::Partition;
use crate::process::{
    CAP_TERMINATION_TIMEOUT, Disposition, ExecutionState, InspectResult, InstanceCensus,
    InstanceVerdict, LaunchAuthority, LaunchError, LaunchedProcessIdentity, ProcessBirth,
    ProcessEventSink, ProcessInstance, ProcessInstanceSource, ProcessOwner, ProcessTreeSnapshot,
    SignalKind, SpawnError, SpawnOptions, SystemProcessInstanceSource, TASK_QUEUE_SHUTDOWN_TIMEOUT,
    TerminationError, TerminationOutcome, exit_status_for_code,
};
#[cfg(not(unix))]
use crate::process::{ManagedProcess, launch_managed};
pub use crate::queue_hold::{
    GroupCensus, GroupMember, HeldPartitionStatus, HoldProof, PlatformObservations, ReasonCode,
    RootObservation, evaluate_hold_proof, verdict_after_owner_recheck,
};
use crate::queue_hold::{HoldPrelude, QueueHoldReason, QueueHoldStatus};
pub use crate::request::ExecutionRequest;
use crate::request::{ActiveTaskSnapshot, DailyCatchupProvenance};

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
    Held {
        partition: Partition,
        reference: String,
        command: Vec<String>,
        reasons: Vec<ReasonCode>,
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

/// The most discovered tasks the queue takes from one replacement.
pub const DISCOVERED_ADMISSION: usize = 4;

/// Reconstructed work. This internal type has no bus representation.
#[derive(Clone)]
pub struct DiscoveredTask {
    pub request: crate::request::BusTaskRequest,
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
    pub held: Vec<HeldPartitionStatus>,
    pub queue_hold: Option<QueueHoldStatus>,
}

/// Summary of a task-queue shutdown captured from the active snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskQueueShutdownReport {
    pub active_count: usize,
    pub forced: bool,
}

pub(crate) trait QueueProcess: Send {
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
    fn last_termination_snapshot(&self) -> Option<ProcessTreeSnapshot> {
        None
    }
    fn terminate_exact_evidence(
        &mut self,
        timeout: Duration,
    ) -> crate::process::TerminationEvidence {
        let result = self.terminate_exact(timeout);
        crate::process::TerminationEvidence {
            result,
            snapshot: self.last_termination_snapshot(),
        }
    }
    #[allow(dead_code)]
    fn terminate_exact_until_evidence(
        &mut self,
        deadline: Instant,
    ) -> crate::process::TerminationEvidence {
        let result = self.terminate_exact_until(deadline);
        crate::process::TerminationEvidence {
            result,
            snapshot: self.last_termination_snapshot(),
        }
    }
    fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        None
    }
    #[allow(dead_code)]
    fn is_quiescent(&self) -> io::Result<bool> {
        Err(io::Error::other("job quiescence is not read on this host"))
    }
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
        #[cfg(unix)]
        {
            self.0.terminate_exact_evidence(timeout).result
        }
        #[cfg(windows)]
        {
            windows_termination(self.0.terminate_exact(timeout))
        }
    }

    fn terminate_exact_until(
        &mut self,
        deadline: Instant,
    ) -> Result<TerminationOutcome, TerminationError> {
        #[cfg(unix)]
        {
            self.0.terminate_exact_until_evidence(deadline).result
        }
        #[cfg(windows)]
        {
            windows_termination(self.0.terminate_exact_until(deadline))
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

    // Windows proves a held partition from Job quiescence, so it keeps the
    // trait's snapshot-free defaults for these three.
    #[cfg(unix)]
    fn last_termination_snapshot(&self) -> Option<ProcessTreeSnapshot> {
        self.0.last_termination_snapshot().cloned()
    }

    #[cfg(unix)]
    fn terminate_exact_evidence(
        &mut self,
        timeout: Duration,
    ) -> crate::process::TerminationEvidence {
        self.0.terminate_exact_evidence(timeout)
    }

    #[cfg(unix)]
    fn terminate_exact_until_evidence(
        &mut self,
        deadline: Instant,
    ) -> crate::process::TerminationEvidence {
        self.0.terminate_exact_until_evidence(deadline)
    }

    fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
        self.0.exact_identity()
    }

    #[allow(dead_code)]
    fn is_quiescent(&self) -> io::Result<bool> {
        #[cfg(windows)]
        {
            self.0.is_quiescent()
        }
        #[cfg(not(windows))]
        {
            Err(io::Error::other("job quiescence is not read on this host"))
        }
    }
}

#[cfg(windows)]
fn windows_termination(
    result: Result<(), crate::process::LaunchError>,
) -> Result<TerminationOutcome, TerminationError> {
    match result {
        Ok(()) => Ok(TerminationOutcome::Graceful { exit_code: None }),
        Err(crate::process::LaunchError::Terminate(error)) => Err(TerminationError::Io(error)),
        Err(error) => Err(TerminationError::Io(io::Error::other(error))),
    }
}

type QueueProcessHandle = Arc<Mutex<Box<dyn QueueProcess>>>;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnfakedObserver;

pub(crate) trait TreeObserver: Send + Sync {
    fn census_group(&self, pgid: i32, deadline: Option<Instant>) -> InstanceCensus;
    fn observe(&self, instance: &ProcessInstance) -> InstanceVerdict;
    fn process_owner(&self, pid: u32) -> ProcessOwner;
    fn signal_exact(
        &self,
        target: ProcessInstance,
        signal: SignalKind,
    ) -> Result<(), TerminationError>;
    #[allow(dead_code)]
    fn job_quiescent(&self) -> Result<bool, io::Error>;
    fn boot_identity(&self) -> Option<String>;
    fn supervisor_identity(&self) -> Option<ProcessInstance>;
    fn descendant_tree(
        &self,
        root: &ProcessInstance,
        owner_uid: u32,
    ) -> Result<Vec<ProcessInstance>, ()>;
}

pub(crate) struct SystemTreeObserver;

impl TreeObserver for SystemTreeObserver {
    fn census_group(&self, pgid: i32, deadline: Option<Instant>) -> InstanceCensus {
        SystemProcessInstanceSource.census_group(pgid, deadline)
    }

    fn observe(&self, instance: &ProcessInstance) -> InstanceVerdict {
        SystemProcessInstanceSource.observe(instance)
    }

    fn process_owner(&self, pid: u32) -> ProcessOwner {
        #[cfg(unix)]
        {
            crate::process::process_owner(pid)
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            ProcessOwner::Absent
        }
    }

    fn signal_exact(
        &self,
        target: ProcessInstance,
        signal: SignalKind,
    ) -> Result<(), TerminationError> {
        #[cfg(unix)]
        {
            crate::process::signal_exact_instance(target, signal, &SystemProcessInstanceSource)
        }
        #[cfg(not(unix))]
        {
            let _ = (target, signal);
            Err(TerminationError::DescendantCoverageUnavailable)
        }
    }

    fn job_quiescent(&self) -> Result<bool, io::Error> {
        Err(io::Error::other("job quiescence is not read on this host"))
    }

    fn boot_identity(&self) -> Option<String> {
        crate::queue_hold_store::current_boot_identity()
    }

    fn supervisor_identity(&self) -> Option<ProcessInstance> {
        #[cfg(unix)]
        {
            match SystemProcessInstanceSource.inspect(std::process::id()) {
                InspectResult::Present { instance, .. } => Some(instance),
                _ => None,
            }
        }
        #[cfg(windows)]
        {
            crate::process::current_windows_process_instance().ok()
        }
    }

    fn descendant_tree(
        &self,
        root: &ProcessInstance,
        owner_uid: u32,
    ) -> Result<Vec<ProcessInstance>, ()> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            match crate::process::exact_descendant_tree(
                *root,
                owner_uid,
                &SystemProcessInstanceSource,
            ) {
                Ok(snapshot) => {
                    let mut res = Vec::new();
                    for d in snapshot.descendants {
                        if let Some(birth) = snapshot.descendant_births.get(&d.pid) {
                            res.push(ProcessInstance {
                                pid: d.pid as u32,
                                birth: *birth,
                            });
                        }
                    }
                    Ok(res)
                }
                Err(_) => Err(()),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (root, owner_uid);
            Err(())
        }
    }
}

#[cfg(test)]
pub(crate) struct TestDefaultTreeObserver;
#[cfg(test)]
impl TreeObserver for TestDefaultTreeObserver {
    fn census_group(&self, _pgid: i32, _deadline: Option<Instant>) -> InstanceCensus {
        std::panic::panic_any(UnfakedObserver);
    }
    fn observe(&self, _instance: &ProcessInstance) -> InstanceVerdict {
        std::panic::panic_any(UnfakedObserver);
    }
    fn process_owner(&self, _pid: u32) -> ProcessOwner {
        std::panic::panic_any(UnfakedObserver);
    }
    fn signal_exact(
        &self,
        _target: ProcessInstance,
        _signal: SignalKind,
    ) -> Result<(), TerminationError> {
        std::panic::panic_any(UnfakedObserver);
    }
    fn job_quiescent(&self) -> Result<bool, io::Error> {
        std::panic::panic_any(UnfakedObserver);
    }
    fn boot_identity(&self) -> Option<String> {
        std::panic::panic_any(UnfakedObserver);
    }
    fn supervisor_identity(&self) -> Option<ProcessInstance> {
        std::panic::panic_any(UnfakedObserver);
    }
    fn descendant_tree(
        &self,
        _root: &ProcessInstance,
        _owner_uid: u32,
    ) -> Result<Vec<ProcessInstance>, ()> {
        std::panic::panic_any(UnfakedObserver);
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum QueueSpawnFailure {
    Clean(SpawnError),
    Live(Box<LaunchAuthority>),
}

#[cfg(unix)]
fn classify_generation_child(
    result: Result<LaunchAuthority, crate::process::GenerationChildError>,
) -> Result<LaunchAuthority, QueueSpawnFailure> {
    match result {
        Ok(authority) => Ok(authority),
        Err(crate::process::GenerationChildError::Unavailable(err)) => {
            let spawn_err = match err {
                LaunchError::SpawnManaged(e) => e,
                LaunchError::CapabilityUnavailable { needed } => {
                    SpawnError::CapabilityUnavailable { needed }
                }
                LaunchError::Spawn(e) => SpawnError::Spawn(e),
                LaunchError::Admission(msg) => SpawnError::Spawn(io::Error::other(format!(
                    "failed to launch generation child: admission failed: {msg}"
                ))),
                _ => SpawnError::Spawn(io::Error::other(format!(
                    "failed to launch generation child: {err}"
                ))),
            };
            Err(QueueSpawnFailure::Clean(spawn_err))
        }
        Err(crate::process::GenerationChildError::Live(authority, error)) => {
            let _ = error;
            Err(QueueSpawnFailure::Live(Box::new(authority)))
        }
    }
}

type QueueProcessSpawner = Arc<
    dyn Fn(Vec<String>, SpawnOptions, Duration) -> Result<QueueProcessHandle, QueueSpawnFailure>
        + Send
        + Sync,
>;

#[cfg(test)]
type CatchupAdmissionCapability = Arc<dyn Fn() -> Result<(), CatchupError> + Send + Sync>;

#[cfg(test)]
type WorkerThreadSpawner =
    Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> + Send + Sync>;

fn spawn_managed_queue_process(
    journal_root: PathBuf,
    command: Vec<String>,
    options: SpawnOptions,
    timeout: Duration,
) -> Result<QueueProcessHandle, QueueSpawnFailure> {
    #[cfg(unix)]
    {
        let launch_id = crate::lifecycle::generate_helper_launch_id("task-worker");
        let res = crate::process::launch_managed_generation_child(
            Disposition::IndependentBoundedHelper { timeout },
            &journal_root,
            launch_id,
            crate::process::ManagedLaunchRequest {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                command,
                options,
            },
        );
        let authority = classify_generation_child(res)?;
        Ok(Arc::new(Mutex::new(Box::new(ManagedQueueProcess(
            authority,
        )))))
    }
    #[cfg(not(unix))]
    {
        let _ = journal_root;
        let authority =
            match launch_managed(Disposition::IndependentBoundedHelper { timeout }, || {
                ManagedProcess::spawn_exact(command, options)
            }) {
                Ok(authority) => authority,
                Err(LaunchError::SpawnManaged(error)) => {
                    return Err(QueueSpawnFailure::Clean(error));
                }
                Err(LaunchError::CapabilityUnavailable { needed }) => {
                    return Err(QueueSpawnFailure::Clean(SpawnError::Spawn(
                        io::Error::other(format!("independent launch requires {needed}")),
                    )));
                }
                Err(error) => {
                    unreachable!(
                        "launch_managed(IndependentBoundedHelper) cannot fail with {error}"
                    )
                }
            };
        Ok(Arc::new(Mutex::new(Box::new(ManagedQueueProcess(
            authority,
        )))))
    }
}

#[cfg(windows)]
fn spawn_windows_queue_process(
    journal_root: PathBuf,
    mut command: Vec<String>,
    options: SpawnOptions,
    timeout: Duration,
    grants: &[crate::process::ReadFileGrant],
) -> Result<QueueProcessHandle, QueueSpawnFailure> {
    let Some(program) = command.first() else {
        return Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand));
    };
    let bin_dir = std::env::current_exe()
        .map_err(|e| QueueSpawnFailure::Clean(SpawnError::Spawn(e)))?
        .parent()
        .ok_or_else(|| {
            QueueSpawnFailure::Clean(SpawnError::Spawn(io::Error::other(
                "queue executable has no parent",
            )))
        })?
        .to_path_buf();
    let journal = bin_dir.join("journal.exe");
    let solstone = bin_dir.join("solstone.exe");
    let named_journal =
        program.eq_ignore_ascii_case("journal") || program.eq_ignore_ascii_case("journal.exe");
    let exact_journal = !named_journal
        && std::fs::canonicalize(program)
            .ok()
            .zip(std::fs::canonicalize(&journal).ok())
            .is_some_and(|(actual, expected)| actual == expected);
    let canonical_journal = command.get(1).is_some_and(|argument| argument == "journal")
        && (program.eq_ignore_ascii_case("solstone")
            || program.eq_ignore_ascii_case("solstone.exe")
            || std::fs::canonicalize(program)
                .ok()
                .zip(std::fs::canonicalize(&solstone).ok())
                .is_some_and(|(actual, expected)| actual == expected));
    if grants.is_empty() || !(named_journal || exact_journal || canonical_journal) {
        // Third-party commands keep their existing no-protocol launch contract.
        return spawn_managed_queue_process(journal_root, command, options, timeout);
    }
    // Bind the queue's closed journal command to this installation's binary,
    // rather than letting a PATH override receive an installation capability.
    command[0] = if canonical_journal {
        &solstone
    } else {
        &journal
    }
    .to_str()
    .ok_or_else(|| {
        QueueSpawnFailure::Clean(SpawnError::Spawn(io::Error::other(
            "journal command path is not Unicode",
        )))
    })?
    .to_owned();
    // launch_managed_request with grants either fails before a process exists
    // or, after launch_windows_job_process, moves the owner into
    // independent_failure or retain_cleanup. Those hard-stop and return a
    // BoundedHelperFailure that owns the job. The job is created with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE and without breakaway, so dropping
    // this error closes the last job handle and the kernel ends the tree.
    // This flatten cannot leave a process for the queue to carry.
    let authority = crate::process::launch_managed_request(
        Disposition::IndependentBoundedHelper { timeout },
        crate::process::ManagedLaunchRequest {
            command,
            options,
            read_file_grants: grants.to_vec(),
        },
    )
    .map_err(|error| QueueSpawnFailure::Clean(SpawnError::Spawn(io::Error::other(error))))?;
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
    /// Windows first-party launches retain these in-process capabilities. Task
    /// submissions, persisted queue data and configuration never carry handles.
    #[cfg(windows)]
    pub read_file_grants: Vec<crate::process::ReadFileGrant>,
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
    /// The supervisor's own resolved sibling binary (see
    /// `solstone_core::supervisor::runtime::preflight_journal_binary`), when one was
    /// validated at startup. A dispatched task whose argv0 is the bare `journal`
    /// re-entrant command is exec'd against this absolute path instead of a `PATH`
    /// lookup — the hosted supervisor's own environment is not guaranteed to carry
    /// the directory `journal` is installed to (a GUI-launched macOS process gets
    /// the system default `PATH`, which excludes `/usr/local/bin`). `partition_for`
    /// recognizes an absolute path whose file name is `solstone-core-journal` or
    /// an absolute `solstone.exe` whose next argument is `journal` as the same command
    /// family for classification, so this changes only how the child is located,
    /// never how the task is classified, capped or deduplicated.
    pub task_binary: Option<PathBuf>,
}

/// A synchronous, per-partition task queue.
///
/// This stays on `std::thread` because ManagedProcess is synchronous and this crate
/// has no async-I/O need; one flat module keeps its tightly coupled state transitions visible.
#[derive(Clone)]
pub struct TaskQueue {
    inner: Arc<QueueInner>,
}

#[derive(Clone)]
pub struct CallableTaskContext {
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    pub priority: WorkPriority,
    pub deadline: Instant,
}
impl CallableTaskContext {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire) || Instant::now() >= self.deadline
    }
}
pub type CallableQueueWorker = Arc<dyn Fn(&[String], CallableTaskContext) -> i32 + Send + Sync>;

pub type CallableAdmission = Arc<dyn Fn(&[String]) -> bool + Send + Sync>;

struct CallableTaskHandle {
    #[cfg(all(test, feature = "full-tests"))]
    reference: String,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    worker: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    observer: Option<thread::JoinHandle<()>>,
    joined: Arc<std::sync::atomic::AtomicBool>,
}

struct QueueInner {
    options: QueueOptions,
    #[cfg(test)]
    work_policy_override: Mutex<Option<WorkPolicy>>,
    state: Mutex<QueueState>,
    reaped: Condvar,
    worker_spawner: Mutex<QueueProcessSpawner>,
    tree_observer: Mutex<Arc<dyn TreeObserver>>,
    recovery_in_progress: std::sync::atomic::AtomicBool,
    #[cfg(any(unix, test))]
    recovery_reap_attempts: std::sync::atomic::AtomicUsize,
    #[cfg(any(windows, test))]
    recovery_quiescence_attempts: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    catchup_admission_capability: Mutex<CatchupAdmissionCapability>,
    #[cfg(test)]
    worker_thread_spawner: Mutex<WorkerThreadSpawner>,
    #[cfg(test)]
    worker_threads: Mutex<Vec<thread::JoinHandle<()>>>,
    #[cfg(test)]
    worker_threads_changed: Condvar,
    #[cfg(test)]
    held_persist_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    callable_worker: Option<CallableQueueWorker>,
    callable_admission: Option<CallableAdmission>,
    daily_talent_roots: Option<(PathBuf, PathBuf)>,
    callable_workers: Mutex<Vec<CallableTaskHandle>>,
    callable_join_failed: Arc<std::sync::atomic::AtomicBool>,
    callable_panics: Arc<Mutex<Vec<Box<dyn std::any::Any + Send>>>>,
}

impl QueueInner {
    fn get_or_read_boot_identity(&self, observer: &dyn TreeObserver) -> Option<String> {
        observer.boot_identity()
    }

    fn get_or_read_supervisor_identity(
        &self,
        observer: &dyn TreeObserver,
    ) -> Option<ProcessInstance> {
        observer.supervisor_identity()
    }

    fn get_or_compute_current_scope(&self, observer: &dyn TreeObserver) -> Option<String> {
        let boot = self.get_or_read_boot_identity(observer);
        let supervisor = self.get_or_read_supervisor_identity(observer)?;
        Some(crate::queue_hold_store::format_scope_dir_name(
            boot.as_deref(),
            &supervisor,
        ))
    }

    #[cfg(test)]
    pub(crate) fn set_held_persist_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self
            .held_persist_hook
            .lock()
            .expect("held persist hook lock poisoned") = hook;
    }
}

struct QueueOptions {
    journal_root: PathBuf,
    cap_resolver: Arc<dyn CapResolver + Send + Sync>,
    process_state_probe: Arc<dyn ProcessStateProbe>,
    queue_sink: Option<Arc<dyn TaskQueueEventSink>>,
    process_sink: Option<Arc<dyn ProcessEventSink>>,
    before_deadline_commit: Option<Arc<dyn Fn() + Send + Sync>>,
    child_environment: BTreeMap<OsString, OsString>,
    task_binary: Option<PathBuf>,
}

#[derive(Clone)]
struct PersistedRecordState {
    scope: String,
    hold_id: String,
    references: Vec<String>,
    command: Vec<String>,
    day: Option<String>,
    scheduler_name: Option<String>,
    root: Option<ProcessInstance>,
    boot_id: Option<String>,
    #[allow(dead_code)]
    supervisor_id: Option<ProcessInstance>,
    owner_uid: u32,
    bound_identities: Vec<LaunchedProcessIdentity>,
    bound_first_terminate_at: BTreeMap<u32, Instant>,
    reasons: Vec<ReasonCode>,
    exit_code: Option<i32>,
    termination_error: Option<String>,
    snapshot_unavailable: bool,
    created_unix: u64,
    root_signalled: bool,
    record_unreadable: bool,
    root_unknown: bool,
}

struct HeldEntry {
    dispatch: Dispatch,
    process: Option<QueueProcessHandle>,
    owner_uid: u32,
    bound_identities: Vec<LaunchedProcessIdentity>,
    bound_first_terminate_at: BTreeMap<u32, Instant>,
    #[allow(dead_code)]
    first_held_at: Instant,
    first_held_at_unix: u64,
    reasons: Vec<ReasonCode>,
    exit_code: i32,
    termination_error: Option<String>,
    snapshot_unavailable: bool,
    persisted: bool,
    records: Vec<PersistedRecordState>,
}

struct QueueState {
    ready: bool,
    shutdown: bool,
    running: BTreeMap<Partition, RunningSlot>,
    held: BTreeMap<Partition, HeldEntry>,
    queue_hold: Option<QueueHoldStatus>,
    queues: BTreeMap<Partition, VecDeque<QueuedEntry>>,
    pending: Vec<Submission>,
    active: BTreeMap<String, ActiveEntry>,
    history: VecDeque<TaskHistoryRecord>,
    timeout_marked: BTreeSet<String>,
    stopped_ticks: BTreeMap<String, u8>,
    termination_attempts: TerminationAttemptRegistry,
    /// When the last older analysis task left; the owner's rest counts from it.
    background_finished_at: Option<Instant>,
}

struct RunningSlot {
    reference: String,
    command: Vec<String>,
    /// Older analysis: it counts against the confidential lane's one task.
    background: bool,
}

#[derive(Clone)]
struct Submission {
    discovered: bool,
    partition: Partition,
    cap: Duration,
    command: Vec<String>,
    reference: String,
    day: Option<String>,
    scheduler_name: Option<String>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
    priority: PriorityIntent,
}

#[derive(Clone)]
struct QueuedEntry {
    sources: Vec<Submission>,
    replaceable: bool,
    references: Vec<String>,
    cap: Duration,
    command: Vec<String>,
    day: Option<String>,
    scheduler_name: Option<String>,
    daily_catchup_provenance: Option<DailyCatchupProvenance>,
    priority: PriorityIntent,
}

#[derive(Clone)]
struct Dispatch {
    sources: Vec<Submission>,
    priority: WorkPriority,
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
    #[allow(dead_code)]
    owner_uid: u32,
    process: QueueProcessHandle,
    termination_error: Option<String>,
    snapshot_unavailable: bool,
    bound_identities: Vec<LaunchedProcessIdentity>,
}

type TerminationAttempt = (String, u64, QueueProcessHandle);
type ShutdownSnapshot = (String, QueueProcessHandle);

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

struct WorkerLease {
    inner: Arc<QueueInner>,
    partition: Partition,
    reference: String,
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        let dispatch = finish_worker(&self.inner, &self.partition, &self.reference);
        for dispatch in dispatch {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
    }
}

fn insert_loaded_record(
    held_entries: &mut BTreeMap<Partition, HeldEntry>,
    partition: &Partition,
    options: &TaskQueueOptions,
    rec_state: PersistedRecordState,
) {
    if let Some(entry) = held_entries.get_mut(partition) {
        for r in &rec_state.references {
            if !entry.dispatch.references.contains(r) {
                entry.dispatch.references.push(r.clone());
            }
        }
        for reason in &rec_state.reasons {
            if !entry.reasons.contains(reason) {
                entry.reasons.push(*reason);
            }
        }
        if rec_state.created_unix < entry.first_held_at_unix {
            entry.first_held_at_unix = rec_state.created_unix;
        }
        entry.records.push(rec_state);
    } else {
        let dispatch = Dispatch {
            sources: Vec::new(),
            submission: Submission {
                discovered: false,
                partition: partition.clone(),
                cap: options.cap_resolver.cap_for(partition),
                command: rec_state.command.clone(),
                reference: rec_state.references.first().cloned().unwrap_or_default(),
                day: rec_state.day.clone(),
                scheduler_name: rec_state.scheduler_name.clone(),
                daily_catchup_provenance: None,
                priority: PriorityIntent::default(),
            },
            references: rec_state.references.clone(),
            daily_catchup_admission: None,
            priority: WorkPriority::default(),
        };
        let reasons = rec_state.reasons.clone();
        let first_held_at_unix = rec_state.created_unix;
        let termination_error = rec_state.termination_error.clone();
        let snapshot_unavailable = rec_state.snapshot_unavailable;
        let exit_code = rec_state.exit_code.unwrap_or(0);
        let owner_uid = rec_state.owner_uid;
        let bound_identities = rec_state.bound_identities.clone();
        held_entries.insert(
            partition.clone(),
            HeldEntry {
                dispatch,
                process: None,
                owner_uid,
                bound_identities,
                bound_first_terminate_at: BTreeMap::new(),
                first_held_at: Instant::now(),
                first_held_at_unix,
                reasons,
                exit_code,
                termination_error,
                snapshot_unavailable,
                persisted: true,
                records: vec![rec_state],
            },
        );
    }
}

impl TaskQueue {
    pub fn new(options: TaskQueueOptions) -> Self {
        Self::construct(options, None, None, None)
    }

    pub fn with_callable_worker(options: TaskQueueOptions, worker: CallableQueueWorker) -> Self {
        Self::construct(options, Some(worker), None, None)
    }

    pub fn with_callable_worker_and_admission(
        options: TaskQueueOptions,
        worker: CallableQueueWorker,
        admission: CallableAdmission,
        daily_talent_roots: Option<(PathBuf, PathBuf)>,
    ) -> Self {
        Self::construct(options, Some(worker), Some(admission), daily_talent_roots)
    }

    fn construct(
        options: TaskQueueOptions,
        callable_worker: Option<CallableQueueWorker>,
        callable_admission: Option<CallableAdmission>,
        daily_talent_roots: Option<(PathBuf, PathBuf)>,
    ) -> Self {
        #[cfg(windows)]
        let spawner: QueueProcessSpawner = {
            let grants = options.read_file_grants.clone();
            let journal_root = options.journal_root.clone();
            Arc::new(move |command, options, timeout| {
                spawn_windows_queue_process(
                    journal_root.clone(),
                    command,
                    options,
                    timeout,
                    &grants,
                )
            })
        };
        #[cfg(not(windows))]
        let spawner: QueueProcessSpawner = {
            let journal_root = options.journal_root.clone();
            Arc::new(move |command, options, timeout| {
                spawn_managed_queue_process(journal_root.clone(), command, options, timeout)
            })
        };
        #[cfg(test)]
        let tree_observer: Arc<dyn TreeObserver> = Arc::new(TestDefaultTreeObserver);
        #[cfg(not(test))]
        let tree_observer: Arc<dyn TreeObserver> = Arc::new(SystemTreeObserver);

        let mut held_entries: BTreeMap<Partition, HeldEntry> = BTreeMap::new();
        let mut queue_hold = None;

        if options.journal_root.is_absolute() {
            let in_flight_dir = crate::queue_hold_store::in_flight_directory(&options.journal_root);
            if in_flight_dir.exists() {
                match std::fs::read_dir(&in_flight_dir) {
                    Err(err) => {
                        queue_hold = Some(QueueHoldStatus {
                            reason: QueueHoldReason::RecordsUnreadable,
                            detail: format!("cannot list in-flight directory: {err}"),
                        });
                    }
                    Ok(scopes) => {
                        for scope_res in scopes {
                            let scope_entry = match scope_res {
                                Ok(e) => e,
                                Err(err) => {
                                    queue_hold = Some(QueueHoldStatus {
                                        reason: QueueHoldReason::RecordsUnreadable,
                                        detail: format!("cannot read in-flight scope entry: {err}"),
                                    });
                                    break;
                                }
                            };
                            let scope_path = scope_entry.path();
                            if !scope_path.is_dir() {
                                continue;
                            }
                            let scope_name = scope_entry.file_name().to_string_lossy().to_string();
                            if crate::queue_hold_store::is_publication_candidate_name(
                                std::ffi::OsStr::new(&scope_name),
                            ) || scope_name.starts_with('.')
                            {
                                continue;
                            }
                            let parsed_scope =
                                crate::queue_hold_store::parse_scope_dir_name(&scope_name);
                            let boot_id = parsed_scope.as_ref().and_then(|s| s.boot_id_hex.clone());
                            let supervisor_id = parsed_scope.as_ref().map(|s| s.supervisor);

                            match std::fs::read_dir(&scope_path) {
                                Err(err) => {
                                    queue_hold = Some(QueueHoldStatus {
                                        reason: QueueHoldReason::RecordsUnreadable,
                                        detail: format!(
                                            "cannot list scope directory {scope_name}: {err}"
                                        ),
                                    });
                                }
                                Ok(records) => {
                                    for record_res in records {
                                        let record_entry = match record_res {
                                            Ok(e) => e,
                                            Err(err) => {
                                                queue_hold = Some(QueueHoldStatus {
                                                    reason: QueueHoldReason::RecordsUnreadable,
                                                    detail: format!(
                                                        "cannot read record entry in {scope_name}: {err}"
                                                    ),
                                                });
                                                break;
                                            }
                                        };
                                        let path = record_entry.path();
                                        let file_name =
                                            record_entry.file_name().to_string_lossy().to_string();
                                        if crate::queue_hold_store::is_publication_candidate_name(
                                            std::ffi::OsStr::new(&file_name),
                                        ) || file_name.starts_with('.')
                                        {
                                            continue;
                                        }
                                        if !file_name.ends_with(".json") {
                                            continue;
                                        }
                                        let file_stem =
                                            file_name.strip_suffix(".json").unwrap_or(&file_name);
                                        let partition_name =
                                            crate::queue_hold_store::hex_decode(file_stem)
                                                .and_then(|bytes| String::from_utf8(bytes).ok())
                                                .unwrap_or_else(|| file_stem.to_owned());
                                        let partition = Partition::new(&partition_name);

                                        if path.is_dir() {
                                            let rec_state = PersistedRecordState {
                                                scope: scope_name.clone(),
                                                hold_id: String::new(),
                                                references: vec![partition_name.clone()],
                                                command: vec![],
                                                day: None,
                                                scheduler_name: None,
                                                root: None,
                                                boot_id: boot_id.clone(),
                                                supervisor_id,
                                                owner_uid: 0,
                                                bound_identities: Vec::new(),
                                                bound_first_terminate_at: BTreeMap::new(),
                                                reasons: vec![ReasonCode::RecordUnreadable],
                                                exit_code: None,
                                                termination_error: None,
                                                snapshot_unavailable: false,
                                                created_unix: unix_seconds(),
                                                root_signalled: false,
                                                record_unreadable: true,
                                                root_unknown: false,
                                            };
                                            insert_loaded_record(
                                                &mut held_entries,
                                                &partition,
                                                &options,
                                                rec_state,
                                            );
                                            continue;
                                        }

                                        match crate::queue_hold_store::read_in_flight_record(&path)
                                        {
                                            Ok(rec) => {
                                                let is_intent_unknown_root = rec.phase == "intent"
                                                    && rec.root.is_none()
                                                    && rec.exit_code.is_none();
                                                let (reasons, root_unknown) =
                                                    if is_intent_unknown_root {
                                                        (vec![ReasonCode::RootUnknown], true)
                                                    } else if rec.reasons.is_empty() {
                                                        (vec![ReasonCode::UnprovenAtStart], false)
                                                    } else {
                                                        (rec.reasons, false)
                                                    };
                                                let bound_identities = rec
                                                    .bound
                                                    .into_iter()
                                                    .map(|b| LaunchedProcessIdentity {
                                                        instance: ProcessInstance {
                                                            pid: b.pid,
                                                            birth: b.birth,
                                                        },
                                                        uid: b.uid,
                                                    })
                                                    .collect();
                                                let rec_state = PersistedRecordState {
                                                    scope: scope_name.clone(),
                                                    hold_id: rec.hold_id,
                                                    references: rec.references,
                                                    command: rec.command,
                                                    day: rec.day,
                                                    scheduler_name: rec.scheduler_name,
                                                    root: rec.root,
                                                    boot_id: boot_id.clone(),
                                                    supervisor_id,
                                                    owner_uid: rec.uid,
                                                    bound_identities,
                                                    bound_first_terminate_at: BTreeMap::new(),
                                                    reasons,
                                                    exit_code: rec.exit_code,
                                                    termination_error: rec.termination_error,
                                                    snapshot_unavailable: rec.snapshot_unavailable,
                                                    created_unix: rec.created_unix,
                                                    root_signalled: false,
                                                    record_unreadable: false,
                                                    root_unknown,
                                                };
                                                insert_loaded_record(
                                                    &mut held_entries,
                                                    &partition,
                                                    &options,
                                                    rec_state,
                                                );
                                            }
                                            Err(_) => {
                                                let rec_state = PersistedRecordState {
                                                    scope: scope_name.clone(),
                                                    hold_id: String::new(),
                                                    references: vec![partition_name.clone()],
                                                    command: vec![],
                                                    day: None,
                                                    scheduler_name: None,
                                                    root: None,
                                                    boot_id: boot_id.clone(),
                                                    supervisor_id,
                                                    owner_uid: 0,
                                                    bound_identities: Vec::new(),
                                                    bound_first_terminate_at: BTreeMap::new(),
                                                    reasons: vec![ReasonCode::RecordUnreadable],
                                                    exit_code: None,
                                                    termination_error: None,
                                                    snapshot_unavailable: false,
                                                    created_unix: unix_seconds(),
                                                    root_signalled: false,
                                                    record_unreadable: true,
                                                    root_unknown: false,
                                                };
                                                insert_loaded_record(
                                                    &mut held_entries,
                                                    &partition,
                                                    &options,
                                                    rec_state,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

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
                    task_binary: options.task_binary,
                },
                state: Mutex::new(QueueState {
                    ready: options.ready,
                    shutdown: false,
                    running: BTreeMap::new(),
                    held: held_entries,
                    queue_hold,
                    queues: BTreeMap::new(),
                    pending: Vec::new(),
                    active: BTreeMap::new(),
                    history: VecDeque::new(),
                    timeout_marked: BTreeSet::new(),
                    stopped_ticks: BTreeMap::new(),
                    termination_attempts: TerminationAttemptRegistry::default(),
                    background_finished_at: None,
                }),
                #[cfg(test)]
                work_policy_override: Mutex::new(None),
                reaped: Condvar::new(),
                worker_spawner: Mutex::new(spawner),
                tree_observer: Mutex::new(tree_observer),
                recovery_in_progress: std::sync::atomic::AtomicBool::new(false),
                #[cfg(any(unix, test))]
                recovery_reap_attempts: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(any(windows, test))]
                recovery_quiescence_attempts: std::sync::atomic::AtomicUsize::new(0),
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
                #[cfg(test)]
                held_persist_hook: Mutex::new(None),
                callable_worker,
                daily_talent_roots,
                callable_admission,
                callable_workers: Mutex::new(Vec::new()),
                callable_join_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                callable_panics: Arc::new(Mutex::new(Vec::new())),
            }),
        }
    }

    pub fn admits_command(&self, command: &[String]) -> bool {
        self.inner
            .callable_admission
            .as_ref()
            .is_none_or(|admit| admit(command))
    }

    pub fn submit(&self, request: ExecutionRequest) -> SubmitOutcome {
        let selection = Selection::read(&self.inner);
        let submission = normalize_request(request, self.inner.options.cap_resolver.as_ref());
        if self
            .inner
            .callable_admission
            .as_ref()
            .is_some_and(|admit| !admit(&submission.command))
        {
            return SubmitOutcome::Rejected;
        }
        let partition = submission.partition.clone();
        let (outcome, dispatch, event) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            if !state.ready {
                state.pending.push(submission);
                (SubmitOutcome::Pending, Vec::new(), None)
            } else if state.shutdown {
                (SubmitOutcome::Rejected, Vec::new(), None)
            } else {
                let reference = submission.reference.clone();
                let outcome = enqueue_locked(&mut state, submission);
                let dispatch = take_ready(&mut state, selection);
                let outcome = if dispatch.iter().any(|d| d.references.contains(&reference)) {
                    SubmitOutcome::Dispatched
                } else {
                    outcome
                };
                let event = Some(queue_changed_event(&state, &partition));
                (outcome, dispatch, event)
            }
        };
        for dispatch in dispatch {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
        emit_queue_event(&self.inner.options.queue_sink, event);
        outcome
    }

    pub fn contains_reference(&self, reference: &str) -> bool {
        self.any_reference(|value| value == reference)
    }

    /// Replace only discovery's unstarted contributions. An ordinary request
    /// coalesced into the same command retains its own priority and authority.
    /// Reservation, execution and safety holds are outside this operation.
    pub fn replace_discovered(&self, mut candidates: Vec<DiscoveredTask>) {
        // Admission can inspect disk, so run it before the queue state lock.
        if let Some(admit) = &self.inner.callable_admission {
            candidates.retain(|candidate| admit(candidate.request.cmd.as_wire()));
        }
        let selection = Selection::read(&self.inner);
        let (dispatches, events) = {
            let mut state = self.inner.state.lock().expect("queue state");
            if state.shutdown {
                return;
            }
            state.pending.retain(|source| !source.discovered);
            let mut changed = state.queues.keys().cloned().collect::<BTreeSet<_>>();
            let mut survivors = Vec::new();
            for queue in state.queues.values_mut() {
                queue.retain_mut(|entry| {
                    if !entry.replaceable || !entry.sources.iter().any(|source| source.discovered) {
                        return true;
                    }
                    survivors.extend(
                        entry
                            .sources
                            .iter()
                            .filter(|source| !source.discovered)
                            .cloned(),
                    );
                    false
                });
            }
            state.queues.retain(|_, queue| !queue.is_empty());
            for source in survivors {
                enqueue_locked(&mut state, source);
            }
            let mut added = 0;
            for candidate in candidates {
                let mut source = normalize_request(
                    ExecutionRequest::Bus(candidate.request),
                    self.inner.options.cap_resolver.as_ref(),
                );
                if state
                    .running
                    .values()
                    .any(|slot| slot.command == source.command)
                    || state
                        .held
                        .values()
                        .any(|held| held.dispatch.submission.command == source.command)
                    || state
                        .queues
                        .values()
                        .flatten()
                        .any(|entry| !entry.replaceable && entry.command == source.command)
                {
                    continue;
                }
                source.discovered = true;
                changed.insert(source.partition.clone());
                if state.ready {
                    enqueue_locked(&mut state, source);
                } else {
                    state.pending.push(source);
                }
                added += 1;
                if added == DISCOVERED_ADMISSION {
                    break;
                }
            }
            let dispatches = take_ready(&mut state, selection);
            changed.extend(
                dispatches
                    .iter()
                    .map(|dispatch| dispatch.submission.partition.clone()),
            );
            let events = changed
                .iter()
                .map(|partition| queue_changed_event(&state, partition))
                .collect::<Vec<_>>();
            (dispatches, events)
        };
        for event in events {
            emit_queue_event(&self.inner.options.queue_sink, Some(event));
        }
        for dispatch in dispatches {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
    }

    /// Whether any pending, queued, running or held task carries a reference
    /// that `matches`.
    pub fn any_reference(&self, matches: impl Fn(&str) -> bool) -> bool {
        let state = self.inner.state.lock().expect("queue state lock poisoned");
        state.pending.iter().any(|entry| matches(&entry.reference))
            || state.running.values().any(|slot| matches(&slot.reference))
            || state.active.keys().any(|value| matches(value))
            || state
                .held
                .values()
                .any(|entry| entry.dispatch.references.iter().any(|value| matches(value)))
            || state
                .queues
                .values()
                .flatten()
                .any(|entry| entry.references.iter().any(|value| matches(value)))
    }

    pub fn set_ready(&self) {
        let selection = Selection::read(&self.inner);
        let (dispatches, events) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            if state.ready {
                return;
            }
            state.ready = true;
            if state.shutdown {
                return;
            }
            let pending = std::mem::take(&mut state.pending);
            let mut changed = BTreeSet::new();
            for submission in pending {
                changed.insert(submission.partition.clone());
                enqueue_locked(&mut state, submission);
            }
            let dispatches = take_ready(&mut state, selection);
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

    /// Resume work held by the owner's processing window without disturbing
    /// shutdown or the queue's independent process-tree safety holds.
    pub fn resume_processing(&self) {
        let selection = Selection::read(&self.inner);
        let dispatches = {
            let mut state = self.inner.state.lock().expect("queue state");
            take_ready(&mut state, selection)
        };
        for dispatch in dispatches {
            start_dispatch(Arc::clone(&self.inner), dispatch);
        }
    }

    pub fn enforce_deadlines(&self, now: Instant) {
        let (has_held, has_queue_hold) = {
            let state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            (!state.held.is_empty(), state.queue_hold.is_some())
        };

        if (has_held || has_queue_hold)
            && self
                .inner
                .recovery_in_progress
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
        {
            struct RecoveryGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for RecoveryGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _guard = RecoveryGuard(&self.inner.recovery_in_progress);

            let _ = catch_unwind(AssertUnwindSafe(|| {
                let observer = self
                    .inner
                    .tree_observer
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();

                let durable = self.inner.options.journal_root.is_absolute();

                // 1. If has_queue_hold on durable journal: try to clear queue-wide hold
                if has_queue_hold && durable {
                    let boot = self.inner.get_or_read_boot_identity(&*observer);
                    let sup = self.inner.get_or_read_supervisor_identity(&*observer);

                    if let Some(supervisor) = sup {
                        let current_scope = crate::queue_hold_store::format_scope_dir_name(
                            boot.as_deref(),
                            &supervisor,
                        );
                        let in_flight_dir = crate::queue_hold_store::in_flight_directory(
                            &self.inner.options.journal_root,
                        );
                        let mut list_ok = true;
                        let mut new_records = Vec::new();

                        if let Ok(scopes) = std::fs::read_dir(&in_flight_dir) {
                            for scope_entry in scopes.flatten() {
                                let scope_path = scope_entry.path();
                                if !scope_path.is_dir() {
                                    continue;
                                }
                                let scope_name =
                                    scope_entry.file_name().to_string_lossy().to_string();
                                if crate::queue_hold_store::is_publication_candidate_name(
                                    std::ffi::OsStr::new(&scope_name),
                                ) || scope_name.starts_with('.')
                                {
                                    continue;
                                }
                                let parsed_scope =
                                    crate::queue_hold_store::parse_scope_dir_name(&scope_name);
                                let boot_id =
                                    parsed_scope.as_ref().and_then(|s| s.boot_id_hex.clone());
                                let supervisor_id = parsed_scope.as_ref().map(|s| s.supervisor);

                                match std::fs::read_dir(&scope_path) {
                                    Err(_) => {
                                        list_ok = false;
                                        break;
                                    }
                                    Ok(entries) => {
                                        for entry in entries.flatten() {
                                            let file_name =
                                                entry.file_name().to_string_lossy().to_string();
                                            if crate::queue_hold_store::is_publication_candidate_name(
                                                std::ffi::OsStr::new(&file_name),
                                            ) || file_name.starts_with('.')
                                            {
                                                continue;
                                            }
                                            if !file_name.ends_with(".json") {
                                                continue;
                                            }
                                            let file_stem = file_name
                                                .strip_suffix(".json")
                                                .unwrap_or(&file_name);
                                            let partition_name =
                                                crate::queue_hold_store::hex_decode(file_stem)
                                                    .and_then(|bytes| String::from_utf8(bytes).ok())
                                                    .unwrap_or_else(|| file_stem.to_owned());
                                            let partition = Partition::new(&partition_name);

                                            let rec_state = if entry.path().is_dir() {
                                                PersistedRecordState {
                                                    scope: scope_name.clone(),
                                                    hold_id: String::new(),
                                                    references: vec![partition_name.clone()],
                                                    command: vec![],
                                                    day: None,
                                                    scheduler_name: None,
                                                    root: None,
                                                    boot_id: boot_id.clone(),
                                                    supervisor_id,
                                                    owner_uid: 0,
                                                    bound_identities: Vec::new(),
                                                    bound_first_terminate_at: BTreeMap::new(),
                                                    reasons: vec![ReasonCode::RecordUnreadable],
                                                    exit_code: None,
                                                    termination_error: None,
                                                    snapshot_unavailable: false,
                                                    created_unix: unix_seconds(),
                                                    root_signalled: false,
                                                    record_unreadable: true,
                                                    root_unknown: false,
                                                }
                                            } else {
                                                match crate::queue_hold_store::read_in_flight_record(
                                                    &entry.path(),
                                                ) {
                                                    Ok(rec) => {
                                                        let is_intent_unknown_root = rec.phase
                                                            == "intent"
                                                            && rec.root.is_none()
                                                            && rec.exit_code.is_none();
                                                        let (reasons, root_unknown) =
                                                            if is_intent_unknown_root {
                                                                (
                                                                    vec![ReasonCode::RootUnknown],
                                                                    true,
                                                                )
                                                            } else if rec.reasons.is_empty() {
                                                                (
                                                                    vec![
                                                                        ReasonCode::UnprovenAtStart,
                                                                    ],
                                                                    false,
                                                                )
                                                            } else {
                                                                (rec.reasons, false)
                                                            };
                                                        let bound_identities = rec
                                                            .bound
                                                            .into_iter()
                                                            .map(|b| LaunchedProcessIdentity {
                                                                instance: ProcessInstance {
                                                                    pid: b.pid,
                                                                    birth: b.birth,
                                                                },
                                                                uid: b.uid,
                                                            })
                                                            .collect();
                                                        PersistedRecordState {
                                                            scope: scope_name.clone(),
                                                            hold_id: rec.hold_id,
                                                            references: rec.references,
                                                            command: rec.command,
                                                            day: rec.day,
                                                            scheduler_name: rec.scheduler_name,
                                                            root: rec.root,
                                                            boot_id: boot_id.clone(),
                                                            supervisor_id,
                                                            owner_uid: rec.uid,
                                                            bound_identities,
                                                            bound_first_terminate_at: BTreeMap::new(
                                                            ),
                                                            reasons,
                                                            exit_code: rec.exit_code,
                                                            termination_error: rec
                                                                .termination_error,
                                                            snapshot_unavailable: rec
                                                                .snapshot_unavailable,
                                                            created_unix: rec.created_unix,
                                                            root_signalled: false,
                                                            record_unreadable: false,
                                                            root_unknown,
                                                        }
                                                    }
                                                    Err(_) => PersistedRecordState {
                                                        scope: scope_name.clone(),
                                                        hold_id: String::new(),
                                                        references: vec![partition_name.clone()],
                                                        command: vec![],
                                                        day: None,
                                                        scheduler_name: None,
                                                        root: None,
                                                        boot_id: boot_id.clone(),
                                                        supervisor_id,
                                                        owner_uid: 0,
                                                        bound_identities: Vec::new(),
                                                        bound_first_terminate_at: BTreeMap::new(),
                                                        reasons: vec![ReasonCode::RecordUnreadable],
                                                        exit_code: None,
                                                        termination_error: None,
                                                        snapshot_unavailable: false,
                                                        created_unix: unix_seconds(),
                                                        root_signalled: false,
                                                        record_unreadable: true,
                                                        root_unknown: false,
                                                    },
                                                }
                                            };
                                            new_records.push((partition, rec_state));
                                        }
                                    }
                                }
                            }
                        } else {
                            list_ok = false;
                        }

                        if list_ok {
                            let scope_dir = in_flight_dir.join(&current_scope);
                            let _ = std::fs::create_dir_all(&scope_dir);
                            let probe_path = scope_dir.join(".tmp_hold_probe_1.tmp");
                            let probe_ok = crate::queue_hold_store::write_in_flight_record(
                                &probe_path,
                                &crate::queue_hold_store::InFlightRecord {
                                    phase: "intent".to_owned(),
                                    hold_id: String::new(),
                                    partition: String::new(),
                                    references: Vec::new(),
                                    command: Vec::new(),
                                    day: None,
                                    scheduler_name: None,
                                    uid: 0,
                                    created_unix: 0,
                                    root: None,
                                    group_id: None,
                                    bound: Vec::new(),
                                    exit_code: None,
                                    reasons: Vec::new(),
                                    termination_error: None,
                                    snapshot_unavailable: false,
                                    held: false,
                                },
                            )
                            .is_ok()
                                && std::fs::remove_file(&probe_path).is_ok();

                            if probe_ok {
                                let selection = Selection::read(&self.inner);
                                let dispatches = {
                                    let mut state =
                                        self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                                    for (partition, rec_state) in new_records {
                                        if !state.held.contains_key(&partition) {
                                            insert_loaded_record(
                                                &mut state.held,
                                                &partition,
                                                &TaskQueueOptions {
                                                    #[cfg(windows)]
                                                    read_file_grants: Vec::new(),
                                                    journal_root: self
                                                        .inner
                                                        .options
                                                        .journal_root
                                                        .clone(),
                                                    cap_resolver: self
                                                        .inner
                                                        .options
                                                        .cap_resolver
                                                        .clone(),
                                                    process_state_probe: self
                                                        .inner
                                                        .options
                                                        .process_state_probe
                                                        .clone(),
                                                    queue_sink: None,
                                                    process_sink: None,
                                                    ready: true,
                                                    before_deadline_commit: None,
                                                    child_environment: BTreeMap::new(),
                                                    task_binary: None,
                                                },
                                                rec_state,
                                            );
                                        }
                                    }
                                    state.queue_hold = None;
                                    take_ready(&mut state, selection)
                                };
                                for dispatch in dispatches {
                                    start_dispatch(Arc::clone(&self.inner), dispatch);
                                }
                            }
                        }
                    }
                }

                // 2. Evaluate held partitions
                let mut held_snapshot = {
                    let state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                    state
                        .held
                        .iter()
                        .map(|(p, h)| {
                            let in_flight = state
                                .termination_attempts
                                .by_reference
                                .contains_key(&h.dispatch.submission.reference);
                            (
                                p.clone(),
                                h.dispatch.clone(),
                                h.process.clone(),
                                h.owner_uid,
                                h.bound_identities.clone(),
                                h.bound_first_terminate_at.clone(),
                                h.reasons.clone(),
                                h.exit_code,
                                h.termination_error.clone(),
                                h.snapshot_unavailable,
                                h.persisted,
                                h.records.clone(),
                                in_flight,
                            )
                        })
                        .collect::<Vec<_>>()
                };

                for (
                    partition,
                    dispatch,
                    proc_handle,
                    owner_uid,
                    bound_identities,
                    mut bound_first_terminate_at,
                    _reasons,
                    exit_code,
                    termination_error,
                    snapshot_unavailable,
                    _persisted,
                    records,
                    in_flight,
                ) in held_snapshot.drain(..)
                {
                    if in_flight {
                        continue;
                    }

                    if let Some(proc_handle) = &proc_handle {
                        #[cfg(any(unix, test))]
                        {
                            self.inner
                                .recovery_reap_attempts
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        #[cfg(any(windows, test))]
                        {
                            self.inner
                                .recovery_quiescence_attempts
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }

                        let mut proc = match proc_handle.try_lock() {
                            Ok(guard) => guard,
                            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                            Err(std::sync::TryLockError::WouldBlock) => {
                                continue;
                            }
                        };

                        #[cfg(any(unix, test))]
                        {
                            let _ = proc.poll();
                        }
                        #[cfg(test)]
                        {
                            drop(proc);
                            let _ = observer.job_quiescent();
                        }
                        #[cfg(all(windows, not(test)))]
                        {
                            let _ = proc.is_quiescent();
                            drop(proc);
                        }

                        let obs = collect_observations(
                            &*observer,
                            Some(proc_handle),
                            owner_uid,
                            &bound_identities,
                        );

                        if let (
                            PlatformObservations::Unix {
                                root: RootObservation::SameLive { .. },
                                ..
                            },
                            proc_handle,
                        ) = (&obs, proc_handle.clone())
                        {
                            let attempt = {
                                let mut state =
                                    self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                                if let Some(token) = state
                                    .termination_attempts
                                    .begin(&dispatch.submission.reference)
                                {
                                    Some((
                                        dispatch.submission.reference.clone(),
                                        token,
                                        proc_handle,
                                    ))
                                } else {
                                    None
                                }
                            };
                            if let Some((reference, token, process)) = attempt {
                                start_termination(
                                    Arc::clone(&self.inner),
                                    reference,
                                    token,
                                    process,
                                    CAP_TERMINATION_TIMEOUT,
                                );
                            }
                        }

                        for ident in &bound_identities {
                            let verdict = observer.observe(&ident.instance);
                            if let InstanceVerdict::SameLive { .. } = verdict {
                                if let Some(first_term) =
                                    bound_first_terminate_at.get(&ident.instance.pid)
                                    && now.saturating_duration_since(*first_term)
                                        >= CAP_TERMINATION_TIMEOUT
                                {
                                    let _ = observer.signal_exact(ident.instance, SignalKind::Kill);
                                } else {
                                    let _ = observer
                                        .signal_exact(ident.instance, SignalKind::Terminate);
                                    bound_first_terminate_at.insert(ident.instance.pid, now);
                                }
                            }
                        }

                        let proof = evaluate_hold_proof(obs, HoldPrelude::default());
                        match proof {
                            HoldProof::Proven { basis } => {
                                {
                                    let mut state =
                                        self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                                    state.held.remove(&partition);
                                }
                                for rec in &records {
                                    let record_path =
                                        crate::queue_hold_store::partition_record_path(
                                            &self.inner.options.journal_root,
                                            &rec.scope,
                                            &partition,
                                        );
                                    let _ = std::fs::remove_file(&record_path);
                                    let audit_record = crate::queue_hold_store::HoldAuditRecord {
                                        event: "released".to_owned(),
                                        hold_id: rec.hold_id.clone(),
                                        partition: partition.as_str().to_owned(),
                                        references: rec.references.clone(),
                                        reasons: rec.reasons.clone(),
                                        termination_error: rec.termination_error.clone(),
                                        snapshot_unavailable: rec.snapshot_unavailable,
                                        basis: Some(basis),
                                    };
                                    let _ = crate::queue_hold_store::append_hold_audit(
                                        &self.inner.options.journal_root,
                                        &audit_record,
                                    );
                                }
                                log_proven_warn_if_needed(
                                    &partition,
                                    &dispatch.submission.reference,
                                    &[],
                                    termination_error.as_deref(),
                                    snapshot_unavailable,
                                );
                                record_completion(
                                    &self.inner,
                                    &dispatch,
                                    exit_code,
                                    exit_status_for_code(exit_code).to_owned(),
                                );
                                let next = finish_worker(
                                    &self.inner,
                                    &partition,
                                    &dispatch.submission.reference,
                                );
                                for next in next {
                                    start_dispatch(Arc::clone(&self.inner), next);
                                }
                            }
                            HoldProof::Unproven { reasons } => {
                                let mut state =
                                    self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                                if let Some(held_entry) = state.held.get_mut(&partition) {
                                    held_entry.reasons = reasons;
                                    held_entry.bound_first_terminate_at = bound_first_terminate_at;
                                }
                            }
                        }
                    } else {
                        // Handle-less hold (loaded across restart)
                        let mut unproven_records = Vec::new();
                        let cur_boot = self.inner.get_or_read_boot_identity(&*observer);

                        for mut rec in records {
                            let reboot = if let (Some(cur), Some(held_hex)) =
                                (cur_boot.as_deref(), rec.boot_id.as_deref())
                            {
                                let cur_hex =
                                    crate::queue_hold_store::hex_encode(cur.trim().as_bytes());
                                cur_hex != held_hex
                            } else {
                                false
                            };

                            #[cfg(windows)]
                            let sup_verdict =
                                rec.supervisor_id.as_ref().map(|sup| observer.observe(sup));
                            #[cfg(not(windows))]
                            let sup_verdict = None;

                            #[cfg(windows)]
                            let platform_windows = true;
                            #[cfg(not(windows))]
                            let platform_windows = false;

                            let prelude = HoldPrelude {
                                worker_ended_without_proof: false,
                                reboot,
                                supervisor_verdict: sup_verdict,
                                platform_windows,
                                root_unknown: rec.root_unknown,
                                record_unreadable: rec.record_unreadable,
                            };

                            if !reboot
                                && !(platform_windows
                                    && matches!(
                                        sup_verdict,
                                        Some(InstanceVerdict::NotSameOrExited)
                                    ))
                                && !rec.root_unknown
                                && !rec.record_unreadable
                                && let Some(root_instance) = &rec.root
                            {
                                let root_verdict = observer.observe(root_instance);
                                if let InstanceVerdict::SameLive { .. } = root_verdict {
                                    if !rec.root_signalled {
                                        match observer.descendant_tree(root_instance, rec.owner_uid)
                                        {
                                            Ok(instances) => {
                                                for inst in instances {
                                                    if inst.pid != root_instance.pid
                                                        && !rec
                                                            .bound_identities
                                                            .iter()
                                                            .any(|b| b.instance.pid == inst.pid)
                                                    {
                                                        rec.bound_identities.push(
                                                            LaunchedProcessIdentity {
                                                                instance: inst,
                                                                uid: rec.owner_uid,
                                                            },
                                                        );
                                                    }
                                                }
                                            }
                                            Err(()) => {
                                                rec.snapshot_unavailable = true;
                                            }
                                        }
                                        let _ = observer
                                            .signal_exact(*root_instance, SignalKind::Terminate);
                                        rec.bound_first_terminate_at.insert(root_instance.pid, now);
                                        rec.root_signalled = true;
                                    } else if let Some(first_term) =
                                        rec.bound_first_terminate_at.get(&root_instance.pid)
                                        && now.saturating_duration_since(*first_term)
                                            >= CAP_TERMINATION_TIMEOUT
                                    {
                                        let _ =
                                            observer.signal_exact(*root_instance, SignalKind::Kill);
                                    }
                                }

                                for ident in &rec.bound_identities {
                                    let verdict = observer.observe(&ident.instance);
                                    if let InstanceVerdict::SameLive { .. } = verdict {
                                        match rec.bound_first_terminate_at.entry(ident.instance.pid)
                                        {
                                            std::collections::btree_map::Entry::Occupied(entry) => {
                                                if now.saturating_duration_since(*entry.get())
                                                    >= CAP_TERMINATION_TIMEOUT
                                                {
                                                    let _ = observer.signal_exact(
                                                        ident.instance,
                                                        SignalKind::Kill,
                                                    );
                                                }
                                            }
                                            std::collections::btree_map::Entry::Vacant(entry) => {
                                                let _ = observer.signal_exact(
                                                    ident.instance,
                                                    SignalKind::Terminate,
                                                );
                                                entry.insert(now);
                                            }
                                        }
                                    }
                                }
                            }

                            #[cfg(unix)]
                            let obs = {
                                let (root_obs, group_id) = if let Some(root_instance) = &rec.root {
                                    let verdict = observer.observe(root_instance);
                                    let owner = observer.process_owner(root_instance.pid);
                                    let verdict =
                                        verdict_after_owner_recheck(verdict, owner, rec.owner_uid);
                                    let root = match verdict {
                                        InstanceVerdict::SameLive { .. } => {
                                            RootObservation::SameLive {
                                                birth: root_instance.birth,
                                            }
                                        }
                                        InstanceVerdict::NotSameOrExited => RootObservation::Gone {
                                            birth_verifiable: root_instance.birth.is_verifiable(),
                                            birth: Some(root_instance.birth),
                                        },
                                        InstanceVerdict::Unverifiable => {
                                            RootObservation::Unverifiable {
                                                birth_verifiable: root_instance
                                                    .birth
                                                    .is_verifiable(),
                                                birth: Some(root_instance.birth),
                                            }
                                        }
                                    };
                                    (root, root_instance.pid)
                                } else {
                                    (
                                        RootObservation::Gone {
                                            birth_verifiable: false,
                                            birth: None,
                                        },
                                        0,
                                    )
                                };

                                let mut bound = Vec::new();
                                for ident in &rec.bound_identities {
                                    let v = observer.observe(&ident.instance);
                                    let owner = observer.process_owner(ident.instance.pid);
                                    let v = verdict_after_owner_recheck(v, owner, rec.owner_uid);
                                    bound.push(v);
                                }

                                let census = observer.census_group(group_id as i32, None);
                                let group = match census {
                                    InstanceCensus::Incomplete(_) => GroupCensus::Incomplete,
                                    InstanceCensus::Complete(entries) => GroupCensus::Complete(
                                        entries
                                            .into_iter()
                                            .map(|e| GroupMember {
                                                pid: e.instance.pid,
                                                pgid: e.pgid,
                                                uid: e.uid,
                                                birth: e.instance.birth,
                                            })
                                            .collect(),
                                    ),
                                };

                                PlatformObservations::Unix {
                                    root: root_obs,
                                    bound,
                                    group,
                                    group_id,
                                    owner_uid: rec.owner_uid,
                                }
                            };

                            #[cfg(windows)]
                            let obs = {
                                let job = observer.job_quiescent().map_err(|_| ());
                                PlatformObservations::Windows { job }
                            };

                            let proof = evaluate_hold_proof(obs, prelude);
                            match proof {
                                HoldProof::Proven { basis } => {
                                    let record_path =
                                        crate::queue_hold_store::partition_record_path(
                                            &self.inner.options.journal_root,
                                            &rec.scope,
                                            &partition,
                                        );
                                    let _ = std::fs::remove_file(&record_path);
                                    let audit_record = crate::queue_hold_store::HoldAuditRecord {
                                        event: "released".to_owned(),
                                        hold_id: rec.hold_id.clone(),
                                        partition: partition.as_str().to_owned(),
                                        references: rec.references.clone(),
                                        reasons: rec.reasons.clone(),
                                        termination_error: rec.termination_error.clone(),
                                        snapshot_unavailable: rec.snapshot_unavailable,
                                        basis: Some(basis),
                                    };
                                    let _ = crate::queue_hold_store::append_hold_audit(
                                        &self.inner.options.journal_root,
                                        &audit_record,
                                    );
                                    for r in &rec.references {
                                        emit_queue_event(
                                            &self.inner.options.queue_sink,
                                            Some(TaskQueueEvent::Stopped {
                                                partition: partition.clone(),
                                                reference: r.clone(),
                                                command: rec.command.clone(),
                                                exit_code: rec.exit_code.unwrap_or(0),
                                            }),
                                        );
                                    }
                                }
                                HoldProof::Unproven { reasons } => {
                                    rec.reasons = reasons;
                                    unproven_records.push(rec);
                                }
                            }
                        }

                        if unproven_records.is_empty() {
                            {
                                let mut state =
                                    self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                                state.held.remove(&partition);
                            }
                            let next = finish_worker(
                                &self.inner,
                                &partition,
                                &dispatch.submission.reference,
                            );
                            for next in next {
                                start_dispatch(Arc::clone(&self.inner), next);
                            }
                        } else {
                            let mut state =
                                self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
                            if let Some(held_entry) = state.held.get_mut(&partition) {
                                let mut merged_reasons = Vec::new();
                                for r in &unproven_records {
                                    for reason in &r.reasons {
                                        if !merged_reasons.contains(reason) {
                                            merged_reasons.push(*reason);
                                        }
                                    }
                                }
                                held_entry.reasons = merged_reasons;
                                held_entry.records = unproven_records;
                            }
                        }
                    }
                }
            }));
        }

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

    fn stop_callable_workers(&self) -> bool {
        let mut workers = self
            .inner
            .callable_workers
            .lock()
            .expect("callable workers lock poisoned");
        for entry in workers.iter() {
            entry
                .stop_flag
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        // is_finished() reports the thread body, not completion of TLS teardown.
        // Only the observer's successful native join authorizes reclamation.
        workers.retain(|entry| !entry.joined.load(std::sync::atomic::Ordering::Acquire));
        !workers.is_empty() || self.callable_workers_failed()
    }

    pub fn callable_workers_failed(&self) -> bool {
        self.inner
            .callable_join_failed
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Only an actual retirement owner may use this unbounded drain. Ordinary
    /// deadline shutdown retains live handles in the queue and reports pending.
    pub fn join_callable_workers(&self) -> Result<(), String> {
        let workers = {
            let mut workers = self
                .inner
                .callable_workers
                .lock()
                .map_err(|_| "callable workers lock poisoned")?;
            for entry in workers.iter() {
                entry
                    .stop_flag
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            std::mem::take(&mut *workers)
        };
        for entry in workers {
            if let Some(observer) = entry.observer
                && let Err(payload) = observer.join()
            {
                record_callable_panic(&self.inner, payload);
            }
            // Observer spawn failure leaves the original handle in this cell.
            let worker = entry
                .worker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            if let Some(worker) = worker
                && let Err(payload) = worker.join()
            {
                record_callable_panic(&self.inner, payload);
            }
        }
        if self
            .inner
            .callable_join_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Err("callable worker failed to join".into())
        } else {
            Ok(())
        }
    }

    pub fn shutdown(&self) -> TaskQueueShutdownReport {
        self.shutdown_with_timeout(TASK_QUEUE_SHUTDOWN_TIMEOUT)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> TaskQueueShutdownReport {
        let (active_count, snapshot, initial_forced): (usize, Vec<ShutdownSnapshot>, bool) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            state.shutdown = true;
            let mut active = state
                .active
                .iter()
                .map(|(reference, active)| (reference.clone(), Arc::clone(&active.process)))
                .collect::<Vec<_>>();
            for entry in state.held.values() {
                if let Some(process) = &entry.process {
                    active.push((
                        entry.dispatch.submission.reference.clone(),
                        Arc::clone(process),
                    ));
                }
            }
            let callable_active_count = self
                .inner
                .callable_workers
                .lock()
                .expect("callable workers lock poisoned")
                .len();
            let active_count = state.active.len() + state.held.len() + callable_active_count;
            let initial_forced = !state.held.is_empty();
            (active_count, active, initial_forced)
        };
        let callable_pending = self.stop_callable_workers();
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
                        .terminate_exact(timeout),
                    Err(TerminationError::ParentGraceTimeout)
                )
            }) {
                threads.push(handle);
            }
        }
        let mut forced = initial_forced || callable_pending;
        for thread in threads {
            forced |= thread.join().unwrap_or(false);
        }
        let deadline = Instant::now() + timeout;
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
        forced |= holds_any(&state, &references);
        TaskQueueShutdownReport {
            active_count,
            forced,
        }
    }

    pub fn shutdown_until(&self, deadline: Instant) -> TaskQueueShutdownReport {
        let (active_count, snapshot, initial_forced): (usize, Vec<ShutdownSnapshot>, bool) = {
            let mut state = self.inner.state.lock().expect("queue state lock poisoned");
            state.shutdown = true;
            let mut active = state
                .active
                .iter()
                .map(|(reference, active)| (reference.clone(), Arc::clone(&active.process)))
                .collect::<Vec<_>>();
            for entry in state.held.values() {
                if let Some(process) = &entry.process {
                    active.push((
                        entry.dispatch.submission.reference.clone(),
                        Arc::clone(process),
                    ));
                }
            }
            let callable_active_count = self
                .inner
                .callable_workers
                .lock()
                .expect("callable workers lock poisoned")
                .len();
            let active_count = state.active.len() + state.held.len() + callable_active_count;
            let initial_forced = !state.held.is_empty();
            (active_count, active, initial_forced)
        };
        let callable_pending = self.stop_callable_workers();
        let references = snapshot
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect::<Vec<_>>();
        let (completed_send, completed_receive) = std::sync::mpsc::channel();
        let mut forced = initial_forced || callable_pending;
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
        forced |= holds_any(&state, &references);
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
        let held = state
            .held
            .iter()
            .map(|(partition, entry)| {
                let mut reasons = Vec::new();
                for r in &entry.reasons {
                    if !reasons.contains(r) {
                        reasons.push(*r);
                    }
                }
                HeldPartitionStatus {
                    partition: partition.clone(),
                    reference: entry.dispatch.submission.reference.clone(),
                    references: entry.dispatch.references.clone(),
                    command: entry.dispatch.submission.command.clone(),
                    reasons,
                    termination_error: entry.termination_error.clone(),
                    snapshot_unavailable: entry.snapshot_unavailable,
                    persisted: entry.persisted,
                    held_since_unix: entry.first_held_at_unix,
                }
            })
            .collect();
        let queue_hold = state.queue_hold.clone();
        TaskQueueStatusSnapshot {
            tasks,
            recent_tasks,
            queues,
            held,
            queue_hold,
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

    pub fn history(&self) -> Vec<TaskHistoryRecord> {
        self.collect_status_snapshot(Instant::now()).recent_tasks
    }

    #[cfg(any(unix, test))]
    #[allow(dead_code)]
    pub(crate) fn recovery_reap_attempts(&self) -> usize {
        self.inner
            .recovery_reap_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(any(windows, test))]
    #[allow(dead_code)]
    pub(crate) fn recovery_quiescence_attempts(&self) -> usize {
        self.inner
            .recovery_quiescence_attempts
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn set_tree_observer(&self, observer: Arc<dyn TreeObserver>) {
        *self
            .inner
            .tree_observer
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = observer;
    }

    #[cfg(test)]
    pub(crate) fn set_held_persist_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.inner.set_held_persist_hook(hook);
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

fn read_work_policy(inner: &QueueInner) -> WorkPolicy {
    #[cfg(test)]
    if let Some(policy) = *inner.work_policy_override.lock().expect("policy override") {
        return policy;
    }
    WorkPolicy::read(&inner.options.journal_root).unwrap_or(WorkPolicy {
        lane: ThinkingLane::Unavailable,
        history_confirmed: false,
        confidential_history_rest: Duration::ZERO,
    })
}

/// What selection reads outside the state lock, so the lock never waits on I/O.
#[derive(Clone, Copy)]
struct Selection {
    allowed: bool,
    today: chrono::NaiveDate,
    spending: WorkPolicy,
    /// No older confidential call holds or awaits the shared slot, and no
    /// failed one asked for a pause.
    background_slot_open: bool,
}

impl Selection {
    fn read(inner: &QueueInner) -> Self {
        let journal = &inner.options.journal_root;
        let spending = read_work_policy(inner);
        Self {
            allowed: crate::processing::processing_allowed(journal),
            today: selection_day(journal),
            spending,
            background_slot_open: spending.lane != ThinkingLane::Confidential
                || confidential_background_open(journal),
        }
    }
}

fn selection_day(journal: &Path) -> chrono::NaiveDate {
    chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .date_naive()
}

fn normalize_request(request: ExecutionRequest, caps: &dyn CapResolver) -> Submission {
    let (mut submission, explicit) = match request {
        ExecutionRequest::Bus(request) => (
            Submission {
                discovered: false,
                cap: caps.cap_for(&request.cmd.partition()),
                partition: request.cmd.partition(),
                command: request.cmd.as_wire().to_vec(),
                reference: request.reference,
                day: request.day,
                scheduler_name: request.scheduler_name,
                daily_catchup_provenance: request.daily_catchup_provenance,
                priority: PriorityIntent::default(),
            },
            request.priority,
        ),
        ExecutionRequest::Scheduled(request) => (
            Submission {
                discovered: false,
                cap: request
                    .max_runtime
                    .filter(|cap| !cap.is_zero())
                    .unwrap_or_else(|| caps.cap_for(&request.cmd.partition())),
                partition: request.cmd.partition(),
                command: request.cmd.as_wire().to_vec(),
                reference: request.reference,
                day: request.day,
                scheduler_name: request.scheduler_name,
                daily_catchup_provenance: None,
                priority: PriorityIntent::default(),
            },
            None,
        ),
    };
    submission.priority = PriorityIntent::new(explicit);
    submission
}

fn enqueue_locked(state: &mut QueueState, submission: Submission) -> SubmitOutcome {
    let queue = state
        .queues
        .entry(submission.partition.clone())
        .or_default();
    if let Some(entry) = queue
        .iter_mut()
        .find(|entry| entry.command == submission.command && entry.cap == submission.cap)
    {
        entry.priority.merge(submission.priority);
        if let Some(source) = entry.sources.iter_mut().find(|source| {
            source.reference == submission.reference && source.discovered == submission.discovered
        }) {
            source.priority.merge(submission.priority);
        } else {
            entry.sources.push(submission.clone());
        }
        if entry.references.contains(&submission.reference) {
            return SubmitOutcome::DuplicateQueuedReference;
        }
        entry.references.push(submission.reference);
        return SubmitOutcome::Coalesced;
    }
    queue.push_back(QueuedEntry {
        sources: vec![submission.clone()],
        replaceable: true,
        cap: submission.cap,
        references: vec![submission.reference],
        command: submission.command,
        day: submission.day,
        scheduler_name: submission.scheduler_name,
        daily_catchup_provenance: submission.daily_catchup_provenance,
        priority: submission.priority,
    });
    SubmitOutcome::Queued
}

// One selector for every wakeup. Held lanes are not candidates. A busy lane's
// highest waiting analysis is a barrier: lower work cannot reserve other lanes
// ahead of it. Independent maintenance remains runnable during that wait.
//
// Older confidential work (bands 4-6) shares one engine call at a time per
// journal. One such task runs at a time, and only while no other call holds or
// awaits that slot, no failed call asked for a pause, and the owner's rest has
// passed. Until then older entries are not candidates, so neither they nor a
// waiting call can hold a lane, or be a barrier, that newer work needs.
fn take_ready(state: &mut QueueState, selection: Selection) -> Vec<Dispatch> {
    let Selection {
        allowed,
        today,
        spending,
        background_slot_open,
    } = selection;
    let mut dispatches = Vec::new();
    if !state.ready || state.shutdown || state.queue_hold.is_some() {
        return dispatches;
    }
    loop {
        let background_waits = spending.lane == ThinkingLane::Confidential
            && (!background_slot_open
                || state.running.values().any(|slot| slot.background)
                || state
                    .background_finished_at
                    .is_some_and(|at| at.elapsed() < spending.confidential_history_rest));
        let analysis = allowed
            .then(|| {
                state
                    .queues
                    .iter()
                    .filter(|(partition, _)| !state.held.contains_key(*partition))
                    .flat_map(|(partition, queue)| {
                        queue.iter().enumerate().filter_map(move |(index, entry)| {
                            let priority = entry.priority.resolve(&entry.command, today);
                            (is_analysis_command(&entry.command)
                                && spending.permits(priority.band)
                                && !(background_waits && priority.band.is_background()))
                            .then(|| (partition.clone(), index, priority))
                        })
                    })
                    .min_by_key(|(_, _, priority)| priority.sort_key())
            })
            .flatten();
        let candidate = analysis
            .filter(|(partition, _, _)| !state.running.contains_key(partition))
            .map(|(partition, index, _)| (partition, index))
            .or_else(|| {
                state.queues.iter().find_map(|(partition, queue)| {
                    (!state.running.contains_key(partition)
                        && !state.held.contains_key(partition)
                        && queue
                            .front()
                            .is_some_and(|entry| !is_analysis_command(&entry.command)))
                    .then(|| (partition.clone(), 0))
                })
            });
        let Some((partition, index)) = candidate else {
            break;
        };
        let entry = state
            .queues
            .get_mut(&partition)
            .expect("selected queue")
            .remove(index)
            .expect("selected entry");
        if state.queues[&partition].is_empty() {
            state.queues.remove(&partition);
        }
        let submission = Submission {
            discovered: false,
            partition: partition.clone(),
            cap: entry.cap,
            command: entry.command,
            reference: entry.references[0].clone(),
            day: entry.day,
            scheduler_name: entry.scheduler_name,
            daily_catchup_provenance: entry.daily_catchup_provenance,
            priority: entry.priority,
        };
        let priority = submission.priority.resolve(&submission.command, today);
        state.running.insert(
            partition,
            RunningSlot {
                reference: submission.reference.clone(),
                command: submission.command.clone(),
                background: is_analysis_command(&submission.command)
                    && priority.band.is_background(),
            },
        );
        dispatches.push(Dispatch {
            sources: entry.sources,
            priority,
            submission,
            references: entry.references,
            daily_catchup_admission: None,
        });
    }
    dispatches
}

fn is_analysis_command(command: &[String]) -> bool {
    let alias = crate::partition::journal_alias_form(command);
    let Some(program) = alias
        .first()
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    if (program.eq_ignore_ascii_case("solstone.exe") || program == "solstone")
        && alias.get(1).is_some_and(|v| v == "journal")
    {
        return alias.get(2).is_some_and(|v| v == "think");
    }
    matches!(program, "journal" | "solstone" | "solstone-core-journal")
        && alias.get(1).is_some_and(|v| v == "think")
}

fn record_callable_panic(inner: &QueueInner, payload: Box<dyn std::any::Any + Send>) {
    inner
        .callable_panics
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(payload);
    inner
        .callable_join_failed
        .store(true, std::sync::atomic::Ordering::Release);
}

fn start_dispatch(inner: Arc<QueueInner>, mut dispatch: Dispatch) {
    if is_analysis_command(&dispatch.submission.command)
        && (!crate::processing::processing_allowed(&inner.options.journal_root)
            || !read_work_policy(&inner).permits(dispatch.priority.band))
    {
        let partition = dispatch.submission.partition.clone();
        requeue_dispatch_at_head(&inner, dispatch);
        let event = queue_changed_event(&inner.state.lock().expect("queue state"), &partition);
        emit_queue_event(&inner.options.queue_sink, Some(event));
        return;
    }
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
                for next in next {
                    start_dispatch(Arc::clone(&inner), next);
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
                for next in next {
                    start_dispatch(Arc::clone(&inner), next);
                }
                return;
            }
        }
    }
    if let Some(callable_worker) = inner.callable_worker.as_ref().cloned() {
        let partition = dispatch.submission.partition.clone();
        let reference = dispatch.submission.reference.clone();
        let worker_inner = Arc::clone(&inner);
        let worker_dispatch = dispatch.clone();
        let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop_flag);
        let deadline = Instant::now() + dispatch.submission.cap;
        let worker_reference = reference.clone();
        let worker_partition = partition.clone();
        // Publish the actual handle under the same lock order used by shutdown.
        // Completion cannot acquire state until registration is finished.
        let state = inner.state.lock().expect("queue state lock poisoned");
        if state.shutdown {
            drop(state);
            record_completion(&inner, &dispatch, -1, "shutdown".to_owned());
            for next in finish_worker(&inner, &partition, &reference) {
                start_dispatch(Arc::clone(&inner), next);
            }
            return;
        }
        let mut workers = inner
            .callable_workers
            .lock()
            .expect("callable workers lock poisoned");
        workers.retain(|entry| !entry.joined.load(std::sync::atomic::Ordering::Acquire));
        let spawned = thread::Builder::new().spawn(move || {
            let _lease = WorkerLease {
                inner: Arc::clone(&worker_inner),
                partition: worker_partition,
                reference: worker_reference,
            };
            let result = catch_unwind(AssertUnwindSafe(|| {
                callable_worker(
                    &worker_dispatch.submission.command,
                    CallableTaskContext {
                        cancelled: Arc::clone(&worker_stop),
                        priority: worker_dispatch.priority,
                        deadline,
                    },
                )
            }));
            let result = match result {
                Ok(code) => Some(code),
                Err(payload) => {
                    record_callable_panic(&worker_inner, payload);
                    None
                }
            };
            let (exit_code, status) = if Instant::now() >= deadline {
                (-1, "timeout")
            } else if worker_stop.load(std::sync::atomic::Ordering::Acquire) {
                (-15, "cancelled")
            } else {
                match result {
                    Some(code) => (code, exit_status_for_code(code)),
                    None => (-1, "error"),
                }
            };
            record_completion(
                &worker_inner,
                &worker_dispatch,
                exit_code,
                status.to_owned(),
            );
            // The registry retains the handle through the actual thread exit.
        });
        match spawned {
            Ok(handle) => {
                let worker = Arc::new(Mutex::new(Some(handle)));
                let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let join_worker = Arc::clone(&worker);
                let join_done = Arc::clone(&joined);
                let join_failed = Arc::clone(&inner.callable_join_failed);
                let panics = Arc::clone(&inner.callable_panics);
                let join = move || {
                    let worker = join_worker.lock().unwrap_or_else(|p| p.into_inner()).take();
                    if let Some(worker) = worker {
                        if let Err(payload) = worker.join() {
                            panics
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push(payload);
                            join_failed.store(true, std::sync::atomic::Ordering::Release);
                        }
                    } else {
                        join_failed.store(true, std::sync::atomic::Ordering::Release);
                    }
                    // This observer retains no job/root owner after actual join.
                    // Dropping its handle during later reclamation cannot detach work.
                    join_done.store(true, std::sync::atomic::Ordering::Release);
                };
                #[cfg(test)]
                let observer = {
                    let spawner = Arc::clone(
                        &inner
                            .worker_thread_spawner
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()),
                    );
                    spawner(Box::new(join))
                };
                #[cfg(not(test))]
                let observer = thread::Builder::new()
                    .name("callable-join".into())
                    .spawn(join);
                let observer = match observer {
                    Ok(handle) => Some(handle),
                    Err(_) => {
                        inner
                            .callable_join_failed
                            .store(true, std::sync::atomic::Ordering::Release);
                        None
                    }
                };
                workers.push(CallableTaskHandle {
                    #[cfg(all(test, feature = "full-tests"))]
                    reference: reference.clone(),
                    stop_flag,
                    worker,
                    observer,
                    joined,
                });
            }
            Err(_) => {
                drop(workers);
                drop(state);
                record_completion(&inner, &dispatch, -1, "spawn_failed".to_owned());
                for next in finish_worker(&inner, &partition, &reference) {
                    start_dispatch(Arc::clone(&inner), next);
                }
                return;
            }
        }
        drop(workers);
        drop(state);
        return;
    }
    let rollback = dispatch.clone();
    let worker_dispatch = dispatch.clone();
    let worker = move || {
        let _lease = WorkerLease {
            inner: worker_inner.clone(),
            partition,
            reference,
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_worker(worker_inner.clone(), worker_dispatch.clone());
        }));
        if result.is_err() {
            let task_uid = current_task_uid();
            let (active_proc, _active_pid) = {
                let state = worker_inner
                    .state
                    .lock()
                    .expect("queue state lock poisoned");
                state
                    .active
                    .get(&worker_dispatch.submission.reference)
                    .map(|a| (Arc::clone(&a.process), a.pid))
                    .unzip()
            };
            if let Some(proc) = active_proc {
                let observer = worker_inner
                    .tree_observer
                    .lock()
                    .expect("tree observer lock poisoned")
                    .clone();
                let scope = worker_inner.get_or_compute_current_scope(&*observer);
                let hold_id = crate::queue_hold_store::generate_hold_id().ok();
                let root_identity = proc
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .exact_identity()
                    .map(|i| i.instance);
                handle_worker_exit(
                    &worker_inner,
                    &worker_dispatch,
                    Some(&proc),
                    task_uid,
                    Vec::new(),
                    -1,
                    true,
                    Some("worker panicked".to_owned()),
                    false,
                    scope.as_deref(),
                    hold_id.as_deref(),
                    root_identity,
                );
            } else {
                record_completion(&worker_inner, &worker_dispatch, -1, "error".to_owned());
            }
        }
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
            for next in next {
                start_dispatch(Arc::clone(&inner), next);
            }
        }
    }
}

fn exec_command(task_binary: Option<&Path>, command: &[String]) -> Vec<String> {
    let alias = crate::partition::journal_alias_form(command);
    match (task_binary, alias.first()) {
        (Some(binary), Some(first)) if first == "journal" => {
            let mut resolved = vec![binary.display().to_string()];
            if binary
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.eq_ignore_ascii_case("solstone")
                        || name.eq_ignore_ascii_case("solstone.exe")
                })
            {
                resolved.push("journal".to_owned());
            }
            resolved.extend(alias.iter().skip(1).cloned());
            resolved
        }
        _ => command.to_vec(),
    }
}

fn current_task_uid() -> u32 {
    #[cfg(unix)]
    {
        nix::unistd::getuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn collect_observations(
    observer: &dyn TreeObserver,
    process: Option<&QueueProcessHandle>,
    task_uid: u32,
    bound_identities: &[LaunchedProcessIdentity],
) -> PlatformObservations {
    #[cfg(unix)]
    {
        let (root_identity, snapshot) = if let Some(proc_handle) = process {
            let proc = proc_handle.lock().unwrap_or_else(|p| p.into_inner());
            (proc.exact_identity(), proc.last_termination_snapshot())
        } else {
            (None, None)
        };
        let (root_obs, group_id) = if let Some(identity) = root_identity {
            let instance = identity.instance;
            let pid = instance.pid;
            let group_id = pid;
            let birth_verifiable = instance.birth.is_verifiable();
            let birth = Some(instance.birth);
            let verdict = observer.observe(&instance);
            let owner = observer.process_owner(pid);
            let verdict = verdict_after_owner_recheck(verdict, owner, task_uid);
            let root = match verdict {
                InstanceVerdict::NotSameOrExited => RootObservation::Gone {
                    birth_verifiable,
                    birth,
                },
                InstanceVerdict::SameLive { .. } => RootObservation::SameLive {
                    birth: instance.birth,
                },
                InstanceVerdict::Unverifiable => RootObservation::Unverifiable {
                    birth_verifiable,
                    birth,
                },
            };
            (root, group_id)
        } else {
            (
                RootObservation::Gone {
                    birth_verifiable: false,
                    birth: None,
                },
                0,
            )
        };

        let mut bound = Vec::new();
        for ident in bound_identities {
            let v = observer.observe(&ident.instance);
            let owner = observer.process_owner(ident.instance.pid);
            let v = verdict_after_owner_recheck(v, owner, task_uid);
            bound.push(v);
        }
        if let Some(snapshot) = snapshot {
            for descendant in snapshot.descendants {
                if descendant.uid != task_uid {
                    continue;
                }
                let pid = descendant.pid as u32;
                let birth = snapshot
                    .descendant_births
                    .get(&descendant.pid)
                    .copied()
                    .unwrap_or_else(ProcessBirth::unknown);
                let instance = ProcessInstance { pid, birth };
                let v = observer.observe(&instance);
                let owner = observer.process_owner(instance.pid);
                let v = verdict_after_owner_recheck(v, owner, task_uid);
                bound.push(v);
            }
        }

        let census = observer.census_group(group_id as i32, None);
        let group = match census {
            InstanceCensus::Incomplete(_) => GroupCensus::Incomplete,
            InstanceCensus::Complete(entries) => GroupCensus::Complete(
                entries
                    .into_iter()
                    .map(|e| GroupMember {
                        pid: e.instance.pid,
                        pgid: e.pgid,
                        uid: e.uid,
                        birth: e.instance.birth,
                    })
                    .collect(),
            ),
        };

        PlatformObservations::Unix {
            root: root_obs,
            bound,
            group,
            group_id,
            owner_uid: task_uid,
        }
    }
    #[cfg(all(not(unix), test))]
    {
        let _ = (process, task_uid, bound_identities);
        let job = observer.job_quiescent().map_err(|_| ());
        PlatformObservations::Windows { job }
    }
    #[cfg(all(not(unix), not(test)))]
    {
        let _ = (observer, task_uid, bound_identities);
        let job = match process {
            Some(handle) => match handle.try_lock() {
                Ok(proc) => proc.is_quiescent().map_err(|_| ()),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    poisoned.into_inner().is_quiescent().map_err(|_| ())
                }
                Err(std::sync::TryLockError::WouldBlock) => Err(()),
            },
            None => Err(()),
        };
        PlatformObservations::Windows { job }
    }
}

fn log_proven_warn_if_needed(
    partition: &Partition,
    reference: &str,
    reasons: &[ReasonCode],
    termination_error: Option<&str>,
    snapshot_unavailable: bool,
) {
    if termination_error.is_some() || snapshot_unavailable {
        let reasons_str = reasons
            .iter()
            .map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let error_or_dash = termination_error.unwrap_or("-");
        log::warn!(
            "task partition {} ref {} proven stopped; prior reasons [{}]; termination_error={}; snapshot_unavailable={}",
            partition.as_str(),
            reference,
            reasons_str,
            error_or_dash,
            snapshot_unavailable
        );
    }
}

fn requeue_dispatch_at_head(inner: &QueueInner, dispatch: Dispatch) {
    let mut state = inner.state.lock().expect("queue state lock poisoned");
    state
        .queues
        .entry(dispatch.submission.partition.clone())
        .or_default()
        .push_front(QueuedEntry {
            sources: dispatch.sources,
            replaceable: false,
            references: dispatch.references,
            cap: dispatch.submission.cap,
            command: dispatch.submission.command,
            day: dispatch.submission.day,
            scheduler_name: dispatch.submission.scheduler_name,
            daily_catchup_provenance: dispatch.submission.daily_catchup_provenance,
            priority: dispatch.submission.priority,
        });
    state.running.remove(&dispatch.submission.partition);
}

#[allow(clippy::too_many_arguments)]
fn handle_worker_exit(
    inner: &Arc<QueueInner>,
    dispatch: &Dispatch,
    process: Option<&QueueProcessHandle>,
    task_uid: u32,
    mut bound_identities: Vec<LaunchedProcessIdentity>,
    exit_code: i32,
    worker_ended_without_proof: bool,
    termination_error: Option<String>,
    snapshot_unavailable: bool,
    scope: Option<&str>,
    hold_id: Option<&str>,
    root: Option<ProcessInstance>,
) {
    let durable = inner.options.journal_root.is_absolute();
    if bound_identities.is_empty()
        && let Some(proc_handle) = process
    {
        let proc = proc_handle.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(snapshot) = proc.last_termination_snapshot() {
            for descendant in snapshot.descendants {
                if descendant.uid == task_uid {
                    let pid = descendant.pid as u32;
                    let birth = snapshot
                        .descendant_births
                        .get(&descendant.pid)
                        .copied()
                        .unwrap_or_else(ProcessBirth::unknown);
                    bound_identities.push(LaunchedProcessIdentity {
                        instance: ProcessInstance { pid, birth },
                        uid: descendant.uid,
                    });
                }
            }
        }
    }
    let observer = inner
        .tree_observer
        .lock()
        .expect("tree observer lock poisoned")
        .clone();
    let obs = collect_observations(&*observer, process, task_uid, &bound_identities);
    let prelude = crate::queue_hold::HoldPrelude {
        worker_ended_without_proof,
        ..Default::default()
    };
    let proof = evaluate_hold_proof(obs, prelude);

    match proof {
        HoldProof::Proven { .. } => {
            if let (Some(scope), true) = (scope, durable) {
                let record_path = crate::queue_hold_store::partition_record_path(
                    &inner.options.journal_root,
                    scope,
                    &dispatch.submission.partition,
                );
                if let Err(e) = std::fs::remove_file(&record_path)
                    && e.kind() != io::ErrorKind::NotFound
                {
                    let reasons = vec![ReasonCode::RecordDeleteFailed];
                    let boot_id = inner.get_or_read_boot_identity(&*observer);
                    let supervisor_id = inner.get_or_read_supervisor_identity(&*observer);
                    let hold_id_str = hold_id.unwrap_or_default().to_owned();
                    let rec_state = PersistedRecordState {
                        scope: scope.to_owned(),
                        hold_id: hold_id_str,
                        references: dispatch.references.clone(),
                        command: dispatch.submission.command.clone(),
                        day: dispatch.submission.day.clone(),
                        scheduler_name: dispatch.submission.scheduler_name.clone(),
                        root,
                        boot_id,
                        supervisor_id,
                        owner_uid: task_uid,
                        bound_identities: bound_identities.clone(),
                        bound_first_terminate_at: BTreeMap::new(),
                        reasons: reasons.clone(),
                        exit_code: Some(exit_code),
                        termination_error: termination_error.clone(),
                        snapshot_unavailable,
                        created_unix: unix_seconds(),
                        root_signalled: false,
                        record_unreadable: false,
                        root_unknown: false,
                    };
                    {
                        let mut state = inner.state.lock().expect("queue state lock poisoned");
                        state.active.remove(&dispatch.submission.reference);
                        state.stopped_ticks.remove(&dispatch.submission.reference);
                        state
                            .termination_attempts
                            .by_reference
                            .remove(&dispatch.submission.reference);
                        state.held.insert(
                            dispatch.submission.partition.clone(),
                            HeldEntry {
                                dispatch: dispatch.clone(),
                                process: process.cloned(),
                                owner_uid: task_uid,
                                bound_identities,
                                bound_first_terminate_at: BTreeMap::new(),
                                first_held_at: Instant::now(),
                                first_held_at_unix: unix_seconds(),
                                reasons: reasons.clone(),
                                exit_code,
                                termination_error,
                                snapshot_unavailable,
                                persisted: true,
                                records: vec![rec_state],
                            },
                        );
                    }
                    for reference in &dispatch.references {
                        emit_queue_event(
                            &inner.options.queue_sink,
                            Some(TaskQueueEvent::Held {
                                partition: dispatch.submission.partition.clone(),
                                reference: reference.clone(),
                                command: dispatch.submission.command.clone(),
                                reasons: reasons.clone(),
                            }),
                        );
                    }
                    inner.reaped.notify_all();
                    return;
                }
            }
            log_proven_warn_if_needed(
                &dispatch.submission.partition,
                &dispatch.submission.reference,
                &[],
                termination_error.as_deref(),
                snapshot_unavailable,
            );
            record_completion(
                inner,
                dispatch,
                exit_code,
                exit_status_for_code(exit_code).to_owned(),
            );
        }
        HoldProof::Unproven { reasons } => {
            let boot_id = inner.get_or_read_boot_identity(&*observer);
            let supervisor_id = inner.get_or_read_supervisor_identity(&*observer);
            let hold_id_str = hold_id.unwrap_or_default().to_owned();
            let mut persisted = false;

            if let (Some(scope), true) = (scope, durable) {
                let record = crate::queue_hold_store::InFlightRecord {
                    phase: "running".to_owned(),
                    hold_id: hold_id_str.clone(),
                    partition: dispatch.submission.partition.as_str().to_owned(),
                    references: dispatch.references.clone(),
                    command: dispatch.submission.command.clone(),
                    day: dispatch.submission.day.clone(),
                    scheduler_name: dispatch.submission.scheduler_name.clone(),
                    uid: task_uid,
                    created_unix: unix_seconds(),
                    root,
                    group_id: root.as_ref().map(|r| r.pid),
                    bound: bound_identities
                        .iter()
                        .map(|b| crate::queue_hold_store::PersistedBoundIdentity {
                            pid: b.instance.pid,
                            birth: b.instance.birth,
                            uid: b.uid,
                        })
                        .collect(),
                    exit_code: Some(exit_code),
                    reasons: reasons.clone(),
                    termination_error: termination_error.clone(),
                    snapshot_unavailable,
                    held: true,
                };
                let record_path = crate::queue_hold_store::partition_record_path(
                    &inner.options.journal_root,
                    scope,
                    &dispatch.submission.partition,
                );
                persisted =
                    crate::queue_hold_store::write_in_flight_record(&record_path, &record).is_ok();

                #[cfg(test)]
                {
                    let hook = inner.held_persist_hook.lock().unwrap().clone();
                    if let Some(hook) = hook {
                        hook();
                    }
                }

                let audit_record = crate::queue_hold_store::HoldAuditRecord {
                    event: "held".to_owned(),
                    hold_id: hold_id_str.clone(),
                    partition: dispatch.submission.partition.as_str().to_owned(),
                    references: dispatch.references.clone(),
                    reasons: reasons.clone(),
                    termination_error: termination_error.clone(),
                    snapshot_unavailable,
                    basis: None,
                };
                let _ = crate::queue_hold_store::append_hold_audit(
                    &inner.options.journal_root,
                    &audit_record,
                );
            }

            let rec_state = PersistedRecordState {
                scope: scope.unwrap_or_default().to_owned(),
                hold_id: hold_id_str,
                references: dispatch.references.clone(),
                command: dispatch.submission.command.clone(),
                day: dispatch.submission.day.clone(),
                scheduler_name: dispatch.submission.scheduler_name.clone(),
                root,
                boot_id,
                supervisor_id,
                owner_uid: task_uid,
                bound_identities: bound_identities.clone(),
                bound_first_terminate_at: BTreeMap::new(),
                reasons: reasons.clone(),
                exit_code: Some(exit_code),
                termination_error: termination_error.clone(),
                snapshot_unavailable,
                created_unix: unix_seconds(),
                root_signalled: false,
                record_unreadable: false,
                root_unknown: false,
            };

            {
                let mut state = inner.state.lock().expect("queue state lock poisoned");
                state.active.remove(&dispatch.submission.reference);
                state.stopped_ticks.remove(&dispatch.submission.reference);
                state
                    .termination_attempts
                    .by_reference
                    .remove(&dispatch.submission.reference);
                state.held.insert(
                    dispatch.submission.partition.clone(),
                    HeldEntry {
                        dispatch: dispatch.clone(),
                        process: process.cloned(),
                        owner_uid: task_uid,
                        bound_identities,
                        bound_first_terminate_at: BTreeMap::new(),
                        first_held_at: Instant::now(),
                        first_held_at_unix: unix_seconds(),
                        reasons: reasons.clone(),
                        exit_code,
                        termination_error,
                        snapshot_unavailable,
                        persisted,
                        records: vec![rec_state],
                    },
                );
            }
            for reference in &dispatch.references {
                emit_queue_event(
                    &inner.options.queue_sink,
                    Some(TaskQueueEvent::Held {
                        partition: dispatch.submission.partition.clone(),
                        reference: reference.clone(),
                        command: dispatch.submission.command.clone(),
                        reasons: reasons.clone(),
                    }),
                );
            }
            inner.reaped.notify_all();
        }
    }
}

fn run_worker(inner: Arc<QueueInner>, dispatch: Dispatch) {
    let primary = dispatch.submission.reference.clone();
    let task_uid = current_task_uid();
    let durable = inner.options.journal_root.is_absolute();

    let (scope, hold_id) = if durable {
        let observer = inner
            .tree_observer
            .lock()
            .expect("tree observer lock poisoned")
            .clone();
        let Some(scope) = inner.get_or_compute_current_scope(&*observer) else {
            requeue_dispatch_at_head(&inner, dispatch);
            let mut state = inner.state.lock().expect("queue state lock poisoned");
            state.queue_hold = Some(QueueHoldStatus {
                reason: QueueHoldReason::RecordsUnavailable,
                detail: "supervisor identity unavailable".to_owned(),
            });
            return;
        };
        let Ok(hold_id) = crate::queue_hold_store::generate_hold_id() else {
            requeue_dispatch_at_head(&inner, dispatch);
            let mut state = inner.state.lock().expect("queue state lock poisoned");
            state.queue_hold = Some(QueueHoldStatus {
                reason: QueueHoldReason::RecordsUnavailable,
                detail: "failed to generate hold id".to_owned(),
            });
            return;
        };

        let record = crate::queue_hold_store::InFlightRecord {
            phase: "intent".to_owned(),
            hold_id: hold_id.clone(),
            partition: dispatch.submission.partition.as_str().to_owned(),
            references: dispatch.references.clone(),
            command: dispatch.submission.command.clone(),
            day: dispatch.submission.day.clone(),
            scheduler_name: dispatch.submission.scheduler_name.clone(),
            uid: task_uid,
            created_unix: unix_seconds(),
            root: None,
            group_id: None,
            bound: Vec::new(),
            exit_code: None,
            reasons: Vec::new(),
            termination_error: None,
            snapshot_unavailable: false,
            held: false,
        };
        let record_path = crate::queue_hold_store::partition_record_path(
            &inner.options.journal_root,
            &scope,
            &dispatch.submission.partition,
        );
        if crate::queue_hold_store::write_in_flight_record(&record_path, &record).is_err() {
            requeue_dispatch_at_head(&inner, dispatch);
            let mut state = inner.state.lock().expect("queue state lock poisoned");
            state.queue_hold = Some(QueueHoldStatus {
                reason: QueueHoldReason::RecordsUnavailable,
                detail: "failed to write in-flight intent record".to_owned(),
            });
            return;
        }

        (Some(scope), Some(hold_id))
    } else {
        (None, None)
    };

    let spawner = Arc::clone(
        &inner
            .worker_spawner
            .lock()
            .expect("queue worker spawner lock poisoned"),
    );
    let timeout = dispatch.submission.cap;
    let spawn_res = spawner(
        exec_command(
            inner.options.task_binary.as_deref(),
            &dispatch.submission.command,
        ),
        SpawnOptions {
            journal_root: inner.options.journal_root.clone(),
            reference: primary.clone(),
            day: dispatch.submission.day.clone(),
            sink: inner.options.process_sink.clone(),
            environment: {
                let mut environment = inner.options.child_environment.clone();
                if is_analysis_command(&dispatch.submission.command) {
                    environment.insert(
                        WORK_PRIORITY_ENV.into(),
                        dispatch.priority.to_string().into(),
                    );
                }
                environment
            },
        },
        timeout,
    );

    let (process, live_failure) = match spawn_res {
        Ok(proc) => (proc, false),
        Err(QueueSpawnFailure::Clean(_)) => {
            if let (Some(scope), true) = (&scope, durable) {
                let record_path = crate::queue_hold_store::partition_record_path(
                    &inner.options.journal_root,
                    scope,
                    &dispatch.submission.partition,
                );
                let _ = std::fs::remove_file(&record_path);
            }
            record_completion(&inner, &dispatch, -1, "error".to_owned());
            return;
        }
        Err(QueueSpawnFailure::Live(authority)) => {
            let proc: QueueProcessHandle =
                Arc::new(Mutex::new(Box::new(ManagedQueueProcess(*authority))));
            (proc, true)
        }
    };

    if live_failure {
        let term_evidence = process
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .terminate_exact_evidence(CAP_TERMINATION_TIMEOUT);
        let term_err = match term_evidence.result {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        let snap_unavail = term_evidence.snapshot.is_none();
        let bound_identities = term_evidence
            .snapshot
            .as_ref()
            .map(|s| {
                s.descendants
                    .iter()
                    .filter(|d| d.uid == task_uid)
                    .map(|d| {
                        let pid = d.pid as u32;
                        let birth = s
                            .descendant_births
                            .get(&d.pid)
                            .copied()
                            .unwrap_or_else(ProcessBirth::unknown);
                        LaunchedProcessIdentity {
                            instance: ProcessInstance { pid, birth },
                            uid: d.uid,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        process.lock().unwrap_or_else(|p| p.into_inner()).cleanup();
        handle_worker_exit(
            &inner,
            &dispatch,
            Some(&process),
            task_uid,
            bound_identities,
            -1,
            false,
            term_err,
            snap_unavail,
            scope.as_deref(),
            hold_id.as_deref(),
            None,
        );
        return;
    }

    let (pid, root_identity) = {
        let proc = process.lock().unwrap_or_else(|p| p.into_inner());
        (proc.pid(), proc.exact_identity().map(|i| i.instance))
    };

    if let (Some(scope), Some(hold_id), true) = (&scope, &hold_id, durable) {
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: hold_id.clone(),
            partition: dispatch.submission.partition.as_str().to_owned(),
            references: dispatch.references.clone(),
            command: dispatch.submission.command.clone(),
            day: dispatch.submission.day.clone(),
            scheduler_name: dispatch.submission.scheduler_name.clone(),
            uid: task_uid,
            created_unix: unix_seconds(),
            root: root_identity,
            group_id: root_identity.as_ref().map(|r| r.pid),
            bound: Vec::new(),
            exit_code: None,
            reasons: Vec::new(),
            termination_error: None,
            snapshot_unavailable: false,
            held: false,
        };
        let record_path = crate::queue_hold_store::partition_record_path(
            &inner.options.journal_root,
            scope,
            &dispatch.submission.partition,
        );
        let _ = crate::queue_hold_store::write_in_flight_record(&record_path, &record);
    }

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
                owner_uid: task_uid,
                process: Arc::clone(&process),
                termination_error: None,
                snapshot_unavailable: false,
                bound_identities: Vec::new(),
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
    let (exit_code, term_err, snap_unavail, bound_identities) = loop {
        let poll_result = catch_unwind(AssertUnwindSafe(|| {
            process.lock().unwrap_or_else(|p| p.into_inner()).poll()
        }));
        match poll_result {
            Ok(Ok(Some(code))) => {
                let (term_err, snap_unavail, bound_identities) = {
                    let state = inner.state.lock().expect("queue state lock poisoned");
                    state
                        .active
                        .get(&primary)
                        .map(|a| {
                            (
                                a.termination_error.clone(),
                                a.snapshot_unavailable,
                                a.bound_identities.clone(),
                            )
                        })
                        .unwrap_or_default()
                };
                break (code, term_err, snap_unavail, bound_identities);
            }
            Ok(Ok(None)) => thread::sleep(POLL_INTERVAL),
            Ok(Err(_)) => {
                let term_evidence = process
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .terminate_exact_evidence(CAP_TERMINATION_TIMEOUT);
                let err = match term_evidence.result {
                    Ok(_) => None,
                    Err(e) => Some(e.to_string()),
                };
                let snap = term_evidence.snapshot.is_none();
                let bound_identities = term_evidence
                    .snapshot
                    .as_ref()
                    .map(|s| {
                        s.descendants
                            .iter()
                            .filter(|d| d.uid == task_uid)
                            .map(|d| {
                                let pid = d.pid as u32;
                                let birth = s
                                    .descendant_births
                                    .get(&d.pid)
                                    .copied()
                                    .unwrap_or_else(ProcessBirth::unknown);
                                LaunchedProcessIdentity {
                                    instance: ProcessInstance { pid, birth },
                                    uid: d.uid,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                break (-1, err, snap, bound_identities);
            }
            Err(_) => {
                handle_worker_exit(
                    &inner,
                    &dispatch,
                    Some(&process),
                    task_uid,
                    Vec::new(),
                    -1,
                    true,
                    None,
                    false,
                    scope.as_deref(),
                    hold_id.as_deref(),
                    root_identity,
                );
                return;
            }
        }
    };
    process.lock().unwrap_or_else(|p| p.into_inner()).cleanup();
    handle_worker_exit(
        &inner,
        &dispatch,
        Some(&process),
        task_uid,
        bound_identities,
        exit_code,
        false,
        term_err,
        snap_unavail,
        scope.as_deref(),
        hold_id.as_deref(),
        root_identity,
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
        if let Err(error) = record_daily_catchup_outcome_with_roots(
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
            inner
                .daily_talent_roots
                .as_ref()
                .map(|(talent, apps)| (talent.as_path(), apps.as_path())),
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

/// Shutdown reports a held reference as forced rather than waiting for it:
/// only `enforce_deadlines` releases a held partition, and the supervisor has
/// stopped calling it by the time it shuts the queue down.
fn holds_any(state: &QueueState, references: &[String]) -> bool {
    state
        .held
        .values()
        .any(|held| references.contains(&held.dispatch.submission.reference))
}

fn finish_worker(inner: &QueueInner, partition: &Partition, reference: &str) -> Vec<Dispatch> {
    let selection = Selection::read(inner);
    let (dispatches, events) = {
        let mut state = inner.state.lock().expect("queue state lock poisoned");
        if state.held.contains_key(partition)
            || state
                .running
                .get(partition)
                .map(|slot| slot.reference.as_str())
                != Some(reference)
        {
            return Vec::new();
        }
        if state
            .running
            .remove(partition)
            .is_some_and(|slot| slot.background)
        {
            state.background_finished_at = Some(Instant::now());
        }
        let dispatches = take_ready(&mut state, selection);
        let mut changed = BTreeSet::from([partition.clone()]);
        changed.extend(dispatches.iter().map(|d| d.submission.partition.clone()));
        let events = changed
            .iter()
            .map(|p| queue_changed_event(&state, p))
            .collect::<Vec<_>>();
        (dispatches, events)
    };
    for event in events {
        emit_queue_event(&inner.options.queue_sink, Some(event));
    }
    dispatches
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
            .map(|slot| slot.reference.clone())
            .or_else(|| {
                state
                    .held
                    .get(partition)
                    .map(|h| h.dispatch.submission.reference.clone())
            }),
        queued_depth: queue.len(),
        queue,
    }
}

fn emit_queue_event(sink: &Option<Arc<dyn TaskQueueEventSink>>, event: Option<TaskQueueEvent>) {
    if let (Some(sink), Some(event)) = (sink, event) {
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
    #[cfg(test)]
    let spawned = {
        let spawner = Arc::clone(
            &inner
                .worker_thread_spawner
                .lock()
                .expect("queue worker-thread spawner lock poisoned"),
        );
        spawner(Box::new(move || {
            terminate_process(&thread_inner, &thread_reference, token, process, timeout)
        }))
    };
    #[cfg(not(test))]
    let spawned = thread::Builder::new().spawn(move || {
        terminate_process(&thread_inner, &thread_reference, token, process, timeout)
    });
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
            let mut state = inner.state.lock().expect("queue state lock poisoned");
            if let Some(active) = state.active.get_mut(&reference) {
                active.termination_error = Some("failed to spawn termination thread".to_owned());
            }
            state.termination_attempts.finish(&reference, token);
        }
    }
}

fn terminate_process(
    inner: &QueueInner,
    reference: &str,
    token: u64,
    process: QueueProcessHandle,
    timeout: Duration,
) {
    let term_evidence = process
        .lock()
        .expect("managed process lock poisoned")
        .terminate_exact_evidence(timeout);
    let term_err = match term_evidence.result {
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    };
    let snap_unavail = term_evidence.snapshot.is_none();
    let bound_identities = term_evidence
        .snapshot
        .as_ref()
        .map(|s| {
            s.descendants
                .iter()
                .map(|d| {
                    let pid = d.pid as u32;
                    let birth = s
                        .descendant_births
                        .get(&d.pid)
                        .copied()
                        .unwrap_or_else(ProcessBirth::unknown);
                    LaunchedProcessIdentity {
                        instance: ProcessInstance { pid, birth },
                        uid: d.uid,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    {
        let mut state = inner.state.lock().expect("queue state lock poisoned");
        if let Some(active) = state.active.get_mut(reference) {
            active.termination_error = term_err;
            active.snapshot_unavailable = snap_unavail;
            active.bound_identities = bound_identities;
        }
        state.termination_attempts.finish(reference, token);
    }
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
    use std::collections::HashMap;
    use std::fs;
    use std::sync::{Barrier, Condvar, mpsc};

    use super::*;
    use crate::cap::{DEFAULT_TASK_MAX_RUNTIME, DefaultCapResolver};
    use crate::process::{
        CensusRow, Descendant, Disposition, ExecutionState, InstanceCensus, InstanceVerdict,
        LaunchedProcessIdentity, ProcessBirth, ProcessInstance, ProcessTreeSnapshot,
    };
    use crate::queue_hold::ReasonCode;
    use crate::request::{BusTaskRequest, DailyCatchupProvenance, TaskArgv};

    fn test_birth(token: u64) -> ProcessBirth {
        #[cfg(target_os = "linux")]
        {
            ProcessBirth::linux(token, 0, 100)
        }
        #[cfg(target_os = "macos")]
        {
            ProcessBirth::macos(token as i64)
        }
        #[cfg(windows)]
        {
            ProcessBirth::windows(token)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            ProcessBirth::linux(token, 0, 100)
        }
    }

    struct FixedCap(u64);
    impl CapResolver for FixedCap {
        fn cap_for(&self, _partition: &Partition) -> Duration {
            Duration::from_secs(self.0)
        }
    }

    #[test]
    fn exec_command_resolves_bare_journal_argv0_to_the_supervisors_sibling_binary() {
        let binary = Path::new("/usr/local/lib/solstone-runtime/bin/solstone-core-journal");
        let submitted = vec![
            "journal".to_owned(),
            "think".to_owned(),
            "-v".to_owned(),
            "--day".to_owned(),
            "20260921".to_owned(),
        ];
        assert_eq!(
            exec_command(Some(binary), &submitted),
            vec![
                binary.display().to_string(),
                "think".to_owned(),
                "-v".to_owned(),
                "--day".to_owned(),
                "20260921".to_owned(),
            ]
        );
        // The wire form used for dedup/classification/history is never mutated in place.
        assert_eq!(submitted[0], "journal");
    }

    #[test]
    fn exec_command_launches_solstone_journal_as_its_journal_alias() {
        let binary = Path::new("/usr/local/lib/solstone-runtime/bin/solstone-core-journal");
        let submitted = ["solstone", "journal", "think", "--day", "20260921"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            exec_command(Some(binary), &submitted),
            vec![
                binary.display().to_string(),
                "think".to_owned(),
                "--day".to_owned(),
                "20260921".to_owned(),
            ]
        );
        assert_eq!(exec_command(None, &submitted), submitted);
    }

    #[test]
    fn exec_command_keeps_the_namespace_for_the_canonical_sibling() {
        let binary = Path::new("/install/bin/solstone.exe");
        for submitted in [
            ["journal", "think", "--day", "20260921"]
                .map(str::to_owned)
                .to_vec(),
            ["solstone", "journal", "think", "--day", "20260921"]
                .map(str::to_owned)
                .to_vec(),
        ] {
            assert_eq!(
                exec_command(Some(binary), &submitted),
                [
                    binary.display().to_string(),
                    "journal".into(),
                    "think".into(),
                    "--day".into(),
                    "20260921".into()
                ]
            );
        }
    }

    #[test]
    fn exec_command_leaves_argv_untouched_without_a_resolved_binary() {
        let submitted = vec!["journal".to_owned(), "heartbeat".to_owned()];
        assert_eq!(exec_command(None, &submitted), submitted);
    }

    #[test]
    fn exec_command_does_not_rewrite_a_command_that_is_not_the_bare_journal_reentry() {
        let binary = Path::new("/usr/local/lib/solstone-runtime/bin/solstone-core-journal");
        let submitted = vec!["/bin/sleep".to_owned(), "60".to_owned()];
        assert_eq!(exec_command(Some(binary), &submitted), submitted);
    }

    #[test]
    fn exec_command_leaves_absolute_solstone_exe_journal_argv_untouched() {
        let binary = Path::new("/install/bin/solstone.exe");
        let submitted = vec![
            "/install/bin/solstone.exe".to_owned(),
            "journal".to_owned(),
            "brain".to_owned(),
            "refresh".to_owned(),
        ];
        let result = exec_command(Some(binary), &submitted);
        assert_eq!(result, submitted);
        assert_eq!(result.iter().filter(|arg| *arg == "journal").count(), 1);
    }

    struct FakeTreeObserver {
        census: Mutex<Option<InstanceCensus>>,
        verdicts: Mutex<BTreeMap<u32, InstanceVerdict>>,
        owners: Mutex<BTreeMap<u32, ProcessOwner>>,
        job_quiescent: Mutex<Option<Result<bool, io::Error>>>,
        signals: Mutex<Vec<(ProcessInstance, SignalKind)>>,
        boot_id: Mutex<Option<String>>,
        supervisor_id: Mutex<Option<ProcessInstance>>,
        descendant_trees: Mutex<BTreeMap<u32, Result<Vec<ProcessInstance>, ()>>>,
    }

    impl Default for FakeTreeObserver {
        fn default() -> Self {
            Self {
                census: Mutex::new(None),
                verdicts: Mutex::new(BTreeMap::new()),
                owners: Mutex::new(BTreeMap::new()),
                job_quiescent: Mutex::new(None),
                signals: Mutex::new(Vec::new()),
                boot_id: Mutex::new(Some("boot".to_owned())),
                supervisor_id: Mutex::new(Some(ProcessInstance {
                    pid: 1,
                    birth: ProcessBirth::linux(1, 0, 100),
                })),
                descendant_trees: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl TreeObserver for FakeTreeObserver {
        fn census_group(&self, _pgid: i32, _deadline: Option<Instant>) -> InstanceCensus {
            self.census
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(InstanceCensus::Complete(Vec::new()))
        }

        fn observe(&self, instance: &ProcessInstance) -> InstanceVerdict {
            self.verdicts
                .lock()
                .unwrap()
                .get(&instance.pid)
                .cloned()
                .unwrap_or(InstanceVerdict::NotSameOrExited)
        }

        fn process_owner(&self, pid: u32) -> ProcessOwner {
            self.owners
                .lock()
                .unwrap()
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| ProcessOwner::Uid(current_task_uid()))
        }

        fn signal_exact(
            &self,
            target: ProcessInstance,
            signal: SignalKind,
        ) -> Result<(), TerminationError> {
            self.signals.lock().unwrap().push((target, signal));
            Ok(())
        }

        fn job_quiescent(&self) -> Result<bool, io::Error> {
            self.job_quiescent
                .lock()
                .unwrap()
                .as_ref()
                .map(|res| match res {
                    Ok(b) => Ok(*b),
                    Err(_) => Err(io::Error::other("fake job query failed")),
                })
                .unwrap_or(Ok(true))
        }

        fn boot_identity(&self) -> Option<String> {
            self.boot_id.lock().unwrap().clone()
        }

        fn supervisor_identity(&self) -> Option<ProcessInstance> {
            *self.supervisor_id.lock().unwrap()
        }

        fn descendant_tree(
            &self,
            root: &ProcessInstance,
            _owner_uid: u32,
        ) -> Result<Vec<ProcessInstance>, ()> {
            self.descendant_trees
                .lock()
                .unwrap()
                .get(&root.pid)
                .cloned()
                .unwrap_or(Ok(vec![]))
        }
    }

    enum Poll {
        Error,
        Panic,
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
        birth: ProcessBirth,
        snapshot: Option<ProcessTreeSnapshot>,
    }

    impl FakeProcess {
        fn idle(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::new(),
                terminate_error: false,
                cleanups,
                birth: test_birth(100),
                snapshot: None,
            }
        }

        #[allow(dead_code)]
        fn poll_error(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::from([Poll::Error]),
                terminate_error: true,
                cleanups,
                birth: test_birth(100),
                snapshot: None,
            }
        }

        fn poll_panic(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::from([Poll::Panic]),
                terminate_error: false,
                cleanups,
                birth: test_birth(100),
                snapshot: None,
            }
        }

        fn complete(pid: u32, cleanups: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                pid,
                polls: VecDeque::from([Poll::Complete(0)]),
                terminate_error: false,
                cleanups,
                birth: test_birth(100),
                snapshot: None,
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
                birth: test_birth(100),
                snapshot: None,
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
                Some(Poll::Panic) => panic!("injected poll panic"),
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

        fn last_termination_snapshot(&self) -> Option<ProcessTreeSnapshot> {
            self.snapshot.clone()
        }

        fn terminate_exact_evidence(
            &mut self,
            timeout: Duration,
        ) -> crate::process::TerminationEvidence {
            let result = self.terminate_exact(timeout);
            crate::process::TerminationEvidence {
                result,
                snapshot: self.snapshot.clone(),
            }
        }

        fn terminate_exact_until_evidence(
            &mut self,
            deadline: Instant,
        ) -> crate::process::TerminationEvidence {
            let result = self.terminate_exact_until(deadline);
            crate::process::TerminationEvidence {
                result,
                snapshot: self.snapshot.clone(),
            }
        }

        fn exact_identity(&self) -> Option<LaunchedProcessIdentity> {
            Some(LaunchedProcessIdentity {
                instance: ProcessInstance {
                    pid: self.pid,
                    birth: self.birth,
                },
                uid: current_task_uid(),
            })
        }
    }

    enum SpawnPlan {
        Failure,
        #[allow(dead_code)]
        LiveFailure(Box<LaunchAuthority>),
        Process(FakeProcess),
        Panic(&'static str),
    }

    fn plan_spawner(plans: VecDeque<SpawnPlan>) -> QueueProcessSpawner {
        let plans = Mutex::new(plans);
        Arc::new(
            move |_, _, _| match plans.lock().expect("fake plans").pop_front() {
                Some(SpawnPlan::Failure) => Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand)),
                Some(SpawnPlan::LiveFailure(authority)) => Err(QueueSpawnFailure::Live(authority)),
                Some(SpawnPlan::Process(process)) => Ok(Arc::new(Mutex::new(Box::new(process)))),
                Some(SpawnPlan::Panic(msg)) => panic!("{msg}"),
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
        let observer = Arc::new(FakeTreeObserver::default());
        queue_with_sink_and_observer(ready, cap, plans, queue_sink, observer)
    }

    fn queue_with_sink_and_observer(
        ready: bool,
        cap: u64,
        plans: VecDeque<SpawnPlan>,
        queue_sink: Option<Arc<dyn TaskQueueEventSink>>,
        observer: Arc<FakeTreeObserver>,
    ) -> TaskQueue {
        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: PathBuf::new(),
            cap_resolver: Arc::new(FixedCap(cap)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink,
            process_sink: None,
            ready,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        *queue.inner.work_policy_override.lock().unwrap() = Some(local_spending());
        queue.set_tree_observer(observer);
        queue.set_worker_spawner(plan_spawner(plans));
        queue
    }

    fn local_spending() -> WorkPolicy {
        WorkPolicy {
            lane: ThinkingLane::Bundled,
            history_confirmed: false,
            confidential_history_rest: Duration::ZERO,
        }
    }

    #[cfg(feature = "full-tests")]
    fn selection(today: chrono::NaiveDate, spending: WorkPolicy) -> Selection {
        Selection {
            allowed: true,
            today,
            spending,
            background_slot_open: true,
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn provider_switch_before_launch_holds_history_until_confirmed() {
        let root = tempfile::tempdir().unwrap();
        set_processing(root.path(), false);
        let queue = processing_test_queue(root.path(), false, VecDeque::new());
        let (sent, received) = mpsc::channel();
        queue.set_worker_spawner(Arc::new(move |_, options, _| {
            sent.send(options.reference).unwrap();
            Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand))
        }));
        queue.submit(ranked_request("history", "--segment", "5:1"));
        let reserved = {
            let mut state = queue.inner.state.lock().unwrap();
            for source in std::mem::take(&mut state.pending) {
                enqueue_locked(&mut state, source);
            }
            state.ready = true;
            take_ready(
                &mut state,
                selection(selection_day(root.path()), local_spending()),
            )
            .pop()
            .unwrap()
        };
        fs::write(
            root.path().join("config/journal.json"),
            serde_json::json!({"providers":{"active":{"provider":"openai","model":"model"}}})
                .to_string(),
        )
        .unwrap();
        start_dispatch(Arc::clone(&queue.inner), reserved);
        assert!(received.try_recv().is_err());
        assert!(queue.contains_reference("history"));
        // A permitted task really reaches the worker while history is held.
        queue.submit(ranked_request("current", "--segment", "1:2"));
        assert_eq!(
            received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap(),
            "current"
        );
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
        assert!(queue.contains_reference("history"));
        fs::write(root.path().join("config/journal.json"), serde_json::json!({"providers":{"active":{"provider":"openai","model":"model"}},"processing":{"history_confirmed":true}}).to_string()).unwrap();
        queue.resume_processing();
        assert_eq!(
            received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap(),
            "history"
        );
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn confidential_history_runs_without_spending_confirmation() {
        use sha2::{Digest, Sha256};
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/journal.json"), serde_json::json!({"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"https://example.test","served_model_id":"model","credential":"test"}},"services":{"confidential":{"endpoint_url":"https://example.test","served_model_id":"model","credential_fingerprint_sha256":format!("{:x}", Sha256::digest(b"test"))}}}).to_string()).unwrap();
        let queue = processing_test_queue(root.path(), false, VecDeque::new());
        let (sent, received) = mpsc::channel();
        queue.set_worker_spawner(Arc::new(move |_, options, _| {
            sent.send(options.reference).unwrap();
            Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand))
        }));
        queue.submit(ranked_request("unconfirmed", "--segment", "4:3"));
        queue.replace_discovered(vec![discovered("confirmed", "4:2")]);
        queue.submit(ranked_request("history", "--segment", "5:1"));
        queue.set_ready();
        let mut started = Vec::new();
        for _ in 0..3 {
            started.push(received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap());
        }
        queue.join_test_workers(3, TEST_TRANSITION_TIMEOUT).unwrap();
        assert_eq!(started, ["unconfirmed", "confirmed", "history"]);
        assert!(!queue.contains_reference("history"));
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
            sources: Vec::new(),
            submission: Submission {
                discovered: false,
                cap: Duration::from_secs(10),
                partition: Partition::new("svc"),
                command: vec!["svc".to_owned()],
                reference: reference.to_owned(),
                day: None,
                scheduler_name: None,
                daily_catchup_provenance: None,
                priority: PriorityIntent::default(),
            },
            references: vec![reference.to_owned()],
            daily_catchup_admission: None,
            priority: WorkPriority::default(),
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
                    owner_uid: current_task_uid(),
                    process,
                    termination_error: None,
                    snapshot_unavailable: false,
                    bound_identities: Vec::new(),
                },
            );
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn ranked_request(reference: &str, mode: &str, priority: &str) -> ExecutionRequest {
        let mut command = vec!["journal", "think", "--day", "20260101"];
        if !mode.is_empty() {
            command.extend([mode, reference]);
        }
        let ExecutionRequest::Bus(mut request) = command_request(&command, reference, None) else {
            unreachable!()
        };
        request.priority = Some(priority.parse().unwrap());
        ExecutionRequest::Bus(request)
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn discovered(reference: &str, priority: &str) -> DiscoveredTask {
        let ExecutionRequest::Bus(request) = ranked_request(reference, "--segment", priority)
        else {
            unreachable!()
        };
        DiscoveredTask { request }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn discovered_work_is_bounded_across_passes_and_preserves_other_producers() {
        let queue = queue(false, 10, VecDeque::new());
        for pass in 0..3 {
            queue.replace_discovered(
                (0..12)
                    .map(|n| discovered(&format!("{pass}-{n}"), "5:1"))
                    .collect(),
            );
            assert_eq!(queue.inner.state.lock().unwrap().pending.len(), 4);
        }
        // Ordinary ownership of the same command survives removal of discovery.
        queue.submit(ranked_request("mixed", "--segment", "5:1"));
        queue.replace_discovered(vec![discovered("mixed", "2:1")]);
        {
            let mut state = queue.inner.state.lock().unwrap();
            for source in std::mem::take(&mut state.pending) {
                enqueue_locked(&mut state, source);
            }
            state.ready = true;
            state.queue_hold = Some(QueueHoldStatus {
                reason: QueueHoldReason::RecordsUnavailable,
                detail: "fixture".into(),
            });
            let entry = &state.queues[&Partition::new("segment")][0];
            assert_eq!(entry.sources.len(), 2);
            assert_eq!(
                entry
                    .priority
                    .resolve(&entry.command, selection_day(Path::new("")))
                    .band as u8,
                2
            );
        }
        queue.replace_discovered(Vec::new());
        let state = queue.inner.state.lock().unwrap();
        let entry = &state.queues[&Partition::new("segment")][0];
        assert_eq!(entry.references, ["mixed"]);
        assert_eq!(entry.sources.len(), 1);
        assert_eq!(
            entry
                .priority
                .resolve(&entry.command, selection_day(Path::new("")))
                .band as u8,
            5
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn discovery_never_replaces_reserved_work_or_adds_after_shutdown() {
        let queue = queue(false, 10, VecDeque::new());
        queue.replace_discovered(vec![discovered("reserved", "5:2")]);
        {
            let mut state = queue.inner.state.lock().unwrap();
            for source in std::mem::take(&mut state.pending) {
                enqueue_locked(&mut state, source);
            }
            state.ready = true;
            let reserved = take_ready(
                &mut state,
                selection(selection_day(Path::new("")), local_spending()),
            );
            assert_eq!(reserved.len(), 1);
        }
        queue.replace_discovered(vec![
            discovered("reserved", "5:2"),
            discovered("next", "5:1"),
        ]);
        {
            let mut state = queue.inner.state.lock().unwrap();
            assert_eq!(
                state.running[&Partition::new("segment")].reference,
                "reserved"
            );
            assert_eq!(state.queues[&Partition::new("segment")].len(), 1);
            assert_eq!(
                state.queues[&Partition::new("segment")][0].references,
                ["next"]
            );
            state.shutdown = true;
        }
        queue.replace_discovered(vec![discovered("late", "1:3")]);
        assert!(!queue.contains_reference("late"));
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn explicit_missing_day_priority_survives_submission_and_queue_selection() {
        use solstone_core_journal_config::work_priority::WorkBand;
        let queue = queue(false, 10, VecDeque::new());
        for (reference, day, segment, priority) in [
            (
                "older-recording",
                "19990101",
                Some("120000_60"),
                "5:19990101120000",
            ),
            ("missing-day", "19990102", None, "5:19990102000000"),
            (
                "own-recording",
                "19990102",
                Some("120000_60"),
                "5:19990102120000",
            ),
            ("remake-day", "19990103", None, "6:19990103000000"),
        ] {
            let mut command = vec!["journal", "think", "--day", day];
            if let Some(segment) = segment {
                command.extend(["--segment", segment]);
            }
            let ExecutionRequest::Bus(mut request) = command_request(&command, reference, None)
            else {
                unreachable!()
            };
            request.priority = Some(priority.parse().unwrap());
            queue.submit(ExecutionRequest::Bus(request));
        }
        let mut state = queue.inner.state.lock().unwrap();
        for submission in std::mem::take(&mut state.pending) {
            enqueue_locked(&mut state, submission);
        }
        state.ready = true;
        let mut order = Vec::new();
        while !state.queues.is_empty() {
            let dispatches = take_ready(
                &mut state,
                selection(selection_day(Path::new("")), local_spending()),
            );
            assert!(!dispatches.is_empty());
            for dispatch in dispatches {
                order.push((dispatch.submission.reference, dispatch.priority.band));
            }
            state.running.clear();
        }
        assert_eq!(
            order,
            [
                ("own-recording".into(), WorkBand::Completing),
                ("missing-day".into(), WorkBand::Completing),
                ("older-recording".into(), WorkBand::Completing),
                ("remake-day".into(), WorkBand::Remake),
            ]
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn ladder_selects_globally_and_completion_wakes_other_lanes() {
        let queue = queue(false, 10, VecDeque::new());
        for (reference, mode, priority) in [
            ("old", "", "6:1"),
            ("missing", "--segment", "5:1"),
            ("import", "--activity", "4:1"),
            ("recent", "--segment", "3:1"),
            ("owner", "--activity", "2:1"),
            ("live-older", "--segment", "1:1"),
            ("live-newer", "--segment", "1:2"),
        ] {
            queue.submit(ranked_request(reference, mode, priority));
        }
        let first = {
            let mut state = queue.inner.state.lock().unwrap();
            for submission in std::mem::take(&mut state.pending) {
                enqueue_locked(&mut state, submission);
            }
            state.ready = true;
            take_ready(
                &mut state,
                selection(selection_day(Path::new("")), local_spending()),
            )
        };
        assert_eq!(
            first
                .iter()
                .map(|d| d.submission.reference.as_str())
                .collect::<Vec<_>>(),
            ["live-newer"]
        );
        let second = finish_worker(&queue.inner, &Partition::new("segment"), "live-newer");
        assert_eq!(
            second
                .iter()
                .map(|d| d.submission.reference.as_str())
                .collect::<Vec<_>>(),
            ["live-older", "owner"]
        );
        // The next segment is waiting behind its running task. Import cannot pass it.
        let third = finish_worker(&queue.inner, &Partition::new("activity"), "owner");
        assert!(third.is_empty());
        let fourth = finish_worker(&queue.inner, &Partition::new("segment"), "live-older");
        assert_eq!(
            fourth
                .iter()
                .map(|d| d.submission.reference.as_str())
                .collect::<Vec<_>>(),
            ["recent", "import"]
        );
        assert!(finish_worker(&queue.inner, &Partition::new("activity"), "import").is_empty());
        let fifth = finish_worker(&queue.inner, &Partition::new("segment"), "recent");
        assert_eq!(
            fifth
                .iter()
                .map(|d| d.submission.reference.as_str())
                .collect::<Vec<_>>(),
            ["missing", "old"]
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn requeued_dispatch_recomputes_derived_priority_and_keeps_owner_intent() {
        let queue = queue(false, 10, VecDeque::new());
        let command = [
            "journal",
            "think",
            "--day",
            "20261011",
            "--segment",
            "120000_300",
        ];
        queue.submit(command_request(&command, "derived", None));
        let date = |day| chrono::NaiveDate::from_ymd_opt(2026, 10, day).unwrap();
        let selected = {
            let mut state = queue.inner.state.lock().unwrap();
            for submission in std::mem::take(&mut state.pending) {
                enqueue_locked(&mut state, submission);
            }
            state.ready = true;
            take_ready(&mut state, selection(date(11), local_spending()))
                .pop()
                .unwrap()
        };
        assert_eq!(selected.priority.to_string(), "1:20261011120000");
        requeue_dispatch_at_head(&queue.inner, selected);
        let mut state = queue.inner.state.lock().unwrap();
        let ExecutionRequest::Bus(mut owner) = command_request(&command, "derived", None) else {
            unreachable!()
        };
        owner.priority = Some("2:20261011120000".parse().unwrap());
        assert_eq!(
            enqueue_locked(
                &mut state,
                normalize_request(
                    ExecutionRequest::Bus(owner),
                    queue.inner.options.cap_resolver.as_ref()
                )
            ),
            SubmitOutcome::DuplicateQueuedReference
        );
        let newer = normalize_request(
            command_request(
                &[
                    "journal",
                    "think",
                    "--day",
                    "20261019",
                    "--segment",
                    "120000_300",
                ],
                "today",
                None,
            ),
            queue.inner.options.cap_resolver.as_ref(),
        );
        enqueue_locked(&mut state, newer);
        let first = take_ready(&mut state, selection(date(19), local_spending()));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].submission.reference, "today");
        assert_eq!(first[0].priority.to_string(), "1:20261019120000");
        state.running.clear();
        let second = take_ready(&mut state, selection(date(19), local_spending()));
        assert_eq!(second[0].priority.to_string(), "2:20261011120000");
        assert_eq!(second[0].references, ["derived"]);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn safety_held_analysis_is_not_a_priority_barrier_for_other_lanes() {
        let queue = queue(false, 10, VecDeque::new());
        queue.submit(ranked_request("held-live", "--segment", "1:1"));
        queue.submit(ranked_request("old-day", "", "6:1"));
        let mut state = queue.inner.state.lock().unwrap();
        for submission in std::mem::take(&mut state.pending) {
            enqueue_locked(&mut state, submission);
        }
        state.ready = true;
        state.held.insert(
            Partition::new("segment"),
            HeldEntry {
                dispatch: dispatch("safety-held"),
                process: None,
                owner_uid: current_task_uid(),
                bound_identities: Vec::new(),
                bound_first_terminate_at: BTreeMap::new(),
                first_held_at: Instant::now(),
                first_held_at_unix: 0,
                reasons: vec![ReasonCode::RootLive],
                exit_code: 0,
                termination_error: None,
                snapshot_unavailable: true,
                persisted: false,
                records: Vec::new(),
            },
        );
        let selected = take_ready(
            &mut state,
            selection(selection_day(Path::new("")), local_spending()),
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].submission.reference, "old-day");
        assert!(state.held.contains_key(&Partition::new("segment")));
        assert_eq!(
            state.queues[&Partition::new("segment")][0].references,
            ["held-live"]
        );
    }

    #[cfg(feature = "full-tests")]
    fn confidential_spending(rest: Duration) -> WorkPolicy {
        WorkPolicy {
            lane: ThinkingLane::Confidential,
            history_confirmed: false,
            confidential_history_rest: rest,
        }
    }

    /// Selects with the confidential lane and the background slot as given.
    #[cfg(feature = "full-tests")]
    fn select_confidential(state: &mut QueueState, rest: Duration, slot_open: bool) -> Vec<String> {
        take_ready(
            state,
            Selection {
                allowed: true,
                today: selection_day(Path::new("")),
                spending: confidential_spending(rest),
                background_slot_open: slot_open,
            },
        )
        .into_iter()
        .map(|dispatch| dispatch.submission.reference)
        .collect()
    }

    #[cfg(feature = "full-tests")]
    fn ready_state(queue: &TaskQueue) -> std::sync::MutexGuard<'_, QueueState> {
        let mut state = queue.inner.state.lock().unwrap();
        state.ready = true;
        state
    }

    /// Each reference is its own work, daily included: unlike `ranked_request`,
    /// the reference stays in the command so same-day daily work does not merge.
    #[cfg(feature = "full-tests")]
    fn enqueue_ranked(queue: &TaskQueue, state: &mut QueueState, work: &[(&str, &str, &str)]) {
        for &(reference, mode, priority) in work {
            let mut command = vec!["journal", "think", "--day", "20260101"];
            command.extend([mode, reference].into_iter().filter(|arg| !arg.is_empty()));
            let ExecutionRequest::Bus(mut request) = command_request(&command, reference, None)
            else {
                unreachable!()
            };
            request.priority = Some(priority.parse().unwrap());
            let request = ExecutionRequest::Bus(request);
            enqueue_locked(
                state,
                normalize_request(request, queue.inner.options.cap_resolver.as_ref()),
            );
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn older_confidential_work_runs_one_task_and_holds_no_lane_newer_work_needs() {
        let queue = queue(false, 10, VecDeque::new());
        let mut state = ready_state(&queue);
        let older = [
            ("import", "--activity", "4:1"),
            ("history", "--segment", "5:1"),
            ("remake", "", "6:1"),
        ];
        enqueue_ranked(&queue, &mut state, &older);
        // Another call holds or awaits the slot: no older task starts at all.
        assert!(select_confidential(&mut state, Duration::ZERO, false).is_empty());
        let newer = [
            ("now", "--segment", "1:2"),
            ("waiting", "--activity", "2:2"),
            ("recent", "", "3:2"),
        ];
        enqueue_ranked(&queue, &mut state, &newer);
        assert_eq!(
            select_confidential(&mut state, Duration::ZERO, false),
            ["now", "waiting", "recent"],
            "newer work submitted after older work waits runs in every lane, in band order"
        );
        state.running.clear();
        // Once the slot opens, older work resumes without a new request, one
        // task at a time and highest band first; newer work still runs beside it.
        for (expected, lane) in [
            ("import", "activity"),
            ("history", "segment"),
            ("remake", "daily"),
        ] {
            assert_eq!(
                select_confidential(&mut state, Duration::ZERO, true),
                [expected]
            );
            assert!(select_confidential(&mut state, Duration::ZERO, true).is_empty());
            let now = format!("now-{lane}");
            enqueue_ranked(&queue, &mut state, &[(&now, "--settle", "1:3")]);
            assert_eq!(select_confidential(&mut state, Duration::ZERO, true), [now]);
            state.running.remove(&Partition::new("flush"));
            state.running.remove(&Partition::new(lane));
        }
        assert!(state.queues.is_empty());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn the_owners_rest_runs_between_older_tasks_and_a_change_applies_at_once() {
        let queue = queue(false, 10, VecDeque::new());
        let mut state = ready_state(&queue);
        state.background_finished_at = Some(Instant::now());
        enqueue_ranked(
            &queue,
            &mut state,
            &[
                ("history", "--segment", "5:1"),
                ("now", "--activity", "1:2"),
                ("recent", "", "3:2"),
            ],
        );
        let rest = Duration::from_secs(60);
        assert_eq!(
            select_confidential(&mut state, rest, true),
            ["now", "recent"],
            "newer work never rests"
        );
        assert!(select_confidential(&mut state, rest, true).is_empty());
        assert_eq!(
            select_confidential(&mut state, Duration::ZERO, true),
            ["history"],
            "a change to zero applies at once"
        );
        state.running.remove(&Partition::new("segment"));
        state.background_finished_at = Instant::now().checked_sub(rest + Duration::from_secs(1));
        enqueue_ranked(&queue, &mut state, &[("remake", "--segment", "6:1")]);
        assert_eq!(select_confidential(&mut state, rest, true), ["remake"]);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn an_owner_request_promotes_waiting_older_work_once() {
        let queue = queue(false, 10, VecDeque::new());
        let mut state = ready_state(&queue);
        enqueue_ranked(&queue, &mut state, &[("history", "--segment", "5:1")]);
        assert!(select_confidential(&mut state, Duration::ZERO, false).is_empty());
        enqueue_ranked(&queue, &mut state, &[("history", "--segment", "1:2")]);
        let dispatched = take_ready(
            &mut state,
            Selection {
                allowed: true,
                today: selection_day(Path::new("")),
                spending: confidential_spending(Duration::ZERO),
                background_slot_open: false,
            },
        );
        assert_eq!(dispatched.len(), 1);
        assert_eq!(dispatched[0].references, ["history"]);
        assert_eq!(dispatched[0].priority.to_string(), "1:2");
        assert!(state.queues.is_empty());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn pending_priority_promotion_deduplicates_before_real_worker_spawn() {
        let queue = queue(false, 10, VecDeque::new());
        let (sent, received) = mpsc::channel();
        queue.set_worker_spawner(Arc::new(move |_, options, _| {
            sent.send((
                options.reference,
                options.environment[std::ffi::OsStr::new(WORK_PRIORITY_ENV)].clone(),
            ))
            .unwrap();
            Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand))
        }));
        queue.submit(ranked_request("older", "--segment", "6:1"));
        queue.submit(ranked_request("newer", "--segment", "5:2"));
        // Same queued command and reference, now explicitly requested by owner.
        queue.submit(ranked_request("older", "--segment", "2:1"));
        queue.set_ready();
        let first = received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap();
        let second = received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap();
        assert_eq!(first, ("older".into(), "2:1".into()));
        assert_eq!(second, ("newer".into(), "5:2".into()));
        queue.join_test_workers(2, TEST_TRANSITION_TIMEOUT).unwrap();
        assert_eq!(queue.history().len(), 2);
        assert!(
            queue
                .collect_queue_counts()
                .values()
                .all(|count| *count == 0)
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn startup_derives_newest_segment_first_and_retains_child_context() {
        let queue = queue(false, 10, VecDeque::new());
        let (sent, received) = mpsc::channel();
        queue.set_worker_spawner(Arc::new(move |_, options, _| {
            sent.send((
                options.reference,
                options.environment[std::ffi::OsStr::new(WORK_PRIORITY_ENV)].clone(),
            ))
            .unwrap();
            Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand))
        }));
        for time in ["090000_300", "120000_300"] {
            queue.submit(command_request(
                &["journal", "think", "--day", "20260101", "--segment", time],
                time,
                None,
            ));
        }
        queue.set_ready();
        assert_eq!(
            received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap(),
            ("120000_300".into(), "5:20260101120000".into())
        );
        assert_eq!(
            received.recv_timeout(TEST_TRANSITION_TIMEOUT).unwrap(),
            ("090000_300".into(), "5:20260101090000".into())
        );
        queue.join_test_workers(2, TEST_TRANSITION_TIMEOUT).unwrap();
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
            priority: None,
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
            priority: None,
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
                    sources: Vec::new(),
                    replaceable: false,
                    cap: Duration::from_secs(10),
                    references: vec!["queued".to_owned()],
                    command: vec!["svc".to_owned()],
                    day: None,
                    scheduler_name: None,
                    daily_catchup_provenance: None,
                    priority: PriorityIntent::default(),
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
                held: Vec::new(),
                queue_hold: None,
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
                    command: Vec::new(),
                    background: false,
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
            held: Vec::new(),
            queue_hold: None,
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
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
        crate::daily_coverage::configure_no_daily_work(journal.path());
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(Arc::new(FakeTreeObserver::default()));
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_worker_spawner(Arc::new(move |_, _, _| {
            spawn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand))
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_worker_spawner(Arc::new(move |_, _, _| {
            spawn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(QueueSpawnFailure::Clean(SpawnError::EmptyCommand))
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: Some(Arc::clone(&sink) as Arc<dyn TaskQueueEventSink>),
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(Arc::new(FakeTreeObserver::default()));
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
    fn file_scoped_index_requests_stay_distinct_while_an_indexer_is_running() {
        let queue = queue(true, 42, VecDeque::new());
        let request = |path: &str| {
            ExecutionRequest::Bus(BusTaskRequest {
                cmd: TaskArgv::from_wire(vec![
                    "journal".to_owned(),
                    "indexer".to_owned(),
                    "--rescan-file".to_owned(),
                    path.to_owned(),
                ])
                .expect("indexer argv"),
                reference: format!("indexer:browser:{}", path.replace('/', ":")),
                day: Some("20260804".to_owned()),
                scheduler_name: None,
                // The live supervisor submits directly to this queue without
                // the older request classifier or its wire flag.
                queue_if_active_cmd_differs: false,
                priority: None,
                daily_catchup_provenance: None,
            })
        };
        let first = "chronicle/20260804/device_browser/120000_1/browser_pages.jsonl";
        let second = "chronicle/20260804/device_browser/120001_1/browser_pages.jsonl";
        let ExecutionRequest::Bus(first_request) = request(first) else {
            unreachable!();
        };
        let partition = first_request.cmd.partition();
        queue
            .inner
            .state
            .lock()
            .expect("queue state")
            .running
            .insert(
                partition.clone(),
                RunningSlot {
                    reference: "already-running-indexer".to_owned(),
                    command: Vec::new(),
                    background: false,
                },
            );

        assert_eq!(queue.submit(request(first)), SubmitOutcome::Queued);
        assert_eq!(queue.submit(request(second)), SubmitOutcome::Queued);
        assert_eq!(
            queue.submit(request(second)),
            SubmitOutcome::DuplicateQueuedReference
        );
        let mut state = queue.inner.state.lock().expect("queue state");
        let pending = state.queues.get(&partition).expect("indexer queue");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].command[3], first);
        assert_eq!(pending[1].command[3], second);
        assert_eq!(pending[0].references, [first_request.reference]);
        state.shutdown = true;
        drop(state);
        assert_eq!(queue.submit(request(first)), SubmitOutcome::Rejected);
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
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: PathBuf::new(),
            cap_resolver: Arc::clone(&resolver) as Arc<dyn CapResolver + Send + Sync>,
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(Arc::new(FakeTreeObserver::default()));
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

    #[test]
    fn hold_reasons_snapshot_wire_and_health_rendering() {
        let queue = queue(true, 10, VecDeque::new());
        let mut state = queue.inner.state.lock().unwrap();
        state.held.insert(
            Partition::new("svc"),
            HeldEntry {
                dispatch: dispatch("held-task"),
                process: Some(Arc::new(Mutex::new(Box::new(FakeProcess::idle(
                    1,
                    Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ))))),
                owner_uid: current_task_uid(),
                bound_identities: Vec::new(),
                bound_first_terminate_at: BTreeMap::new(),
                first_held_at: Instant::now(),
                first_held_at_unix: 0,
                reasons: vec![ReasonCode::RootLive, ReasonCode::GroupMemberLive],
                exit_code: 0,
                termination_error: None,
                snapshot_unavailable: false,
                persisted: false,
                records: Vec::new(),
            },
        );
        drop(state);

        queue.submit(request("queued-behind-safety-hold"));
        queue.resume_processing();
        assert!(queue.contains_reference("queued-behind-safety-hold"));
        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snapshot.held.len(), 1);
        assert_eq!(snapshot.held[0].partition, Partition::new("svc"));
        assert_eq!(snapshot.held[0].reference, "held-task");
        assert_eq!(
            snapshot.held[0].reasons,
            vec![ReasonCode::RootLive, ReasonCode::GroupMemberLive]
        );
    }

    fn processing_test_queue(root: &Path, ready: bool, plans: VecDeque<SpawnPlan>) -> TaskQueue {
        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: root.to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(Arc::new(FakeTreeObserver::default()));
        queue.set_worker_spawner(plan_spawner(plans));
        queue
    }

    fn set_processing(root: &Path, deferred: bool) {
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            serde_json::json!({"providers":{"active":{"provider":"local"}},"processing":{
                "mode":if deferred {"deferred"} else {"realtime"},
                "gate":{"time_window":{"enabled":false},"display_powersave":{"enabled":false}}
            }})
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn processing_gate_preserves_priority_deduplication_and_safety_holds() {
        for ready in [false, true] {
            let root = tempfile::tempdir().unwrap();
            set_processing(root.path(), true);
            let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let queue = processing_test_queue(
                root.path(),
                ready,
                VecDeque::from([
                    SpawnPlan::Process(FakeProcess::complete(1, Arc::clone(&cleanups))),
                    SpawnPlan::Process(FakeProcess::complete(2, Arc::clone(&cleanups))),
                    SpawnPlan::Process(FakeProcess::complete(3, Arc::clone(&cleanups))),
                ]),
            );
            let command = ["solstone", "journal", "think", "--day", "20260101"];
            queue.submit(command_request(&command, "first", None));
            if !ready {
                queue.resume_processing();
                queue.set_ready();
            }
            assert_eq!(
                queue.submit(command_request(&command, "first", None)),
                SubmitOutcome::DuplicateQueuedReference
            );
            assert_eq!(
                queue.submit(command_request(&command, "coalesced", None)),
                SubmitOutcome::Coalesced
            );
            queue.submit(command_request(
                &["journal", "think", "--day", "20260102"],
                "second",
                None,
            ));
            // Maintenance is still admitted in its own partition.
            queue.submit(command_request(
                &["journal", "indexer", "refresh"],
                "maintenance",
                None,
            ));
            queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
            let snapshot = queue.collect_status_snapshot(Instant::now());
            assert_eq!(snapshot.recent_tasks.len(), 1);
            assert_eq!(snapshot.recent_tasks[0].reference, "maintenance");
            assert!(queue.contains_reference("coalesced"));
            assert!(!crate::catchup::catchup_state_path(root.path()).exists());
            set_processing(root.path(), false);
            // Neither shutdown nor a queue-wide forensic hold may be bypassed.
            {
                queue.inner.state.lock().unwrap().shutdown = true;
            }
            queue.resume_processing();
            assert!(queue.contains_reference("first"));
            {
                let mut state = queue.inner.state.lock().unwrap();
                assert!(state.running.is_empty());
                state.shutdown = false;
                state.queue_hold = Some(QueueHoldStatus {
                    reason: QueueHoldReason::RecordsUnreadable,
                    detail: "fixture".into(),
                });
            }
            queue.resume_processing();
            {
                let mut state = queue.inner.state.lock().unwrap();
                assert!(state.running.is_empty());
                state.queue_hold = None;
            }
            queue.resume_processing();
            queue.join_test_workers(2, TEST_TRANSITION_TIMEOUT).unwrap();
            let snapshot = queue.collect_status_snapshot(Instant::now());
            assert_eq!(
                snapshot
                    .recent_tasks
                    .iter()
                    .map(|task| task.reference.as_str())
                    .collect::<Vec<_>>(),
                vec!["maintenance", "second", "first"]
            );
            assert!(!queue.contains_reference("coalesced"));
        }
    }

    #[test]
    fn processing_gate_holds_follower_before_catchup_admission_then_resumes_once() {
        let root = tempfile::tempdir().unwrap();
        set_processing(root.path(), false);
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let queue = processing_test_queue(
            root.path(),
            true,
            VecDeque::from([
                gated_plan(
                    Arc::clone(&arrived),
                    Arc::clone(&release),
                    Arc::clone(&cleanups),
                ),
                SpawnPlan::Process(FakeProcess::complete(2, Arc::clone(&cleanups))),
            ]),
        );
        queue.submit(command_request(
            &["journal", "think", "--day", "20260101"],
            "running",
            None,
        ));
        arrived.wait();
        let day = "20260102";
        queue.submit(command_request(
            &["journal", "think", "--day", day],
            "follower",
            Some(DailyCatchupProvenance { day: day.into() }),
        ));
        set_processing(root.path(), true);
        release.wait();
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
        assert!(queue.contains_reference("follower"));
        assert_eq!(
            queue
                .collect_status_snapshot(Instant::now())
                .recent_tasks
                .len(),
            1
        );
        assert!(!crate::catchup::catchup_state_path(root.path()).exists());
        // Keep the production admission record, but use the existing capability seam.
        queue.set_catchup_admission_capability(Arc::new(|| Ok(())));
        set_processing(root.path(), false);
        queue.resume_processing();
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
        queue.resume_processing();
        assert!(!queue.contains_reference("follower"));
        assert_eq!(
            queue
                .collect_status_snapshot(Instant::now())
                .recent_tasks
                .len(),
            2
        );
        assert!(crate::catchup::catchup_state_path(root.path()).exists());
    }

    #[test]
    fn processing_gate_recognizes_native_thinking_forms_only() {
        for command in [
            vec!["solstone", "journal", "think"],
            vec!["journal", "think"],
            vec!["/bin/solstone-core-journal", "think"],
            vec!["/bin/solstone.exe", "journal", "think"],
            vec!["/bin/solstone", "journal", "think"],
        ] {
            assert!(is_analysis_command(
                &command.into_iter().map(str::to_owned).collect::<Vec<_>>()
            ));
        }
        assert!(!is_analysis_command(&[
            "journal".into(),
            "indexer".into(),
            "refresh".into()
        ]));
    }

    #[test]
    fn clean_worker_exit_releases_partition_and_dispatches_follower() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([
                gated_plan(
                    Arc::clone(&arrived),
                    Arc::clone(&release),
                    Arc::clone(&cleanups),
                ),
                SpawnPlan::Process(FakeProcess::complete(2, Arc::clone(&cleanups))),
            ]),
            None,
            observer,
        );

        assert_eq!(queue.submit(request("sentinel")), SubmitOutcome::Dispatched);
        arrived.wait();
        let follower = queue.submit(request("follower"));
        release.wait();
        assert_eq!(follower, SubmitOutcome::Queued);

        queue
            .join_test_workers(2, TEST_TRANSITION_TIMEOUT)
            .expect("workers");

        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert!(snapshot.held.is_empty());
        assert_eq!(snapshot.recent_tasks.len(), 2);
        assert_eq!(snapshot.recent_tasks[0].reference, "sentinel");
        assert_eq!(snapshot.recent_tasks[1].reference, "follower");
    }

    #[test]
    fn queue_tests_never_reach_the_host_observer() {
        let obs = TestDefaultTreeObserver;
        let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
            obs.observe(&ProcessInstance {
                pid: 1,
                birth: test_birth(1),
            });
        }));
        assert!(res.is_err());
    }

    #[test]
    fn termination_evidence_reaches_the_queue() {
        let queue = queue(true, 10, VecDeque::new());
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut fake = FakeProcess::idle(1, cleanups);
        fake.terminate_error = true;
        let mut births = HashMap::new();
        births.insert(2, test_birth(200));
        fake.snapshot = Some(ProcessTreeSnapshot {
            parent_pid: 1,
            parent_pgid: Some(1),
            descendants: vec![Descendant {
                pid: 2,
                ppid: 1,
                pgid: Some(1),
                uid: current_task_uid(),
            }],
            descendant_births: births,
        });
        let proc_handle: QueueProcessHandle = Arc::new(Mutex::new(Box::new(fake)));
        add_active_process(&queue, "t1", Arc::clone(&proc_handle));

        terminate_process(
            &queue.inner,
            "t1",
            0,
            proc_handle,
            Duration::from_millis(10),
        );

        let state = queue.inner.state.lock().unwrap();
        let active = state.active.get("t1").unwrap();
        assert!(active.termination_error.is_some());
        assert!(!active.snapshot_unavailable);
        assert_eq!(active.bound_identities.len(), 1);
        assert_eq!(active.bound_identities[0].instance.pid, 2);
    }

    #[test]
    fn spawn_error_mapping_separates_post_spawn_failures() {
        let clean_queue = queue(true, 10, VecDeque::from([SpawnPlan::Failure]));
        clean_queue.submit(request("clean-fail"));
        clean_queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .unwrap();
        let snap = clean_queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
        assert_eq!(snap.recent_tasks[0].exit_status, "error");

        #[cfg(unix)]
        {
            let observer = Arc::new(FakeTreeObserver::default());
            observer.verdicts.lock().unwrap().insert(
                1,
                InstanceVerdict::SameLive {
                    execution: ExecutionState::Running,
                },
            );
            let authority =
                LaunchAuthority::without_process(Disposition::IndependentBoundedHelper {
                    timeout: Duration::from_secs(10),
                });
            let live_queue = queue_with_sink_and_observer(
                true,
                10,
                VecDeque::from([SpawnPlan::LiveFailure(Box::new(authority))]),
                None,
                observer,
            );
            live_queue.submit(request("live-fail"));
            live_queue
                .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
                .unwrap();
            let snap = live_queue.collect_status_snapshot(Instant::now());
            assert_eq!(snap.held.len(), 1);
            assert_eq!(snap.held[0].reference, "live-fail");
        }
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn poll_error_with_live_root_holds_partition() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            1,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(FakeProcess::poll_error(1, cleanups))]),
            None,
            observer,
        );
        assert_eq!(queue.submit(request("poll-err")), SubmitOutcome::Dispatched);
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();

        let snap = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snap.held.len(), 1);
        assert_eq!(snap.held[0].reference, "poll-err");
        assert_eq!(snap.held[0].reasons, vec![ReasonCode::RootLive]);
    }

    #[test]
    fn poll_error_with_clean_observations_releases_despite_termination_error() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(FakeProcess::poll_error(1, cleanups))]),
            None,
            observer,
        );
        assert_eq!(
            queue.submit(request("poll-err-clean")),
            SubmitOutcome::Dispatched
        );
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();

        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
        assert_eq!(snap.recent_tasks.len(), 1);
        assert_eq!(snap.recent_tasks[0].reference, "poll-err-clean");
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn ordinary_exit_with_live_group_member_holds_partition() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        *observer.census.lock().unwrap() = Some(InstanceCensus::Complete(vec![CensusRow {
            instance: ProcessInstance {
                pid: 10,
                birth: test_birth(500),
            },
            uid: current_task_uid(),
            ppid: 1,
            pgid: 1,
            execution: ExecutionState::Running,
        }]));

        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(FakeProcess::complete(
                1,
                Arc::clone(&cleanups),
            ))]),
            None,
            Arc::clone(&observer),
        );

        assert_eq!(
            queue.submit(request("group-task")),
            SubmitOutcome::Dispatched
        );
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();

        let snap = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snap.held.len(), 1);
        assert_eq!(snap.held[0].reasons, vec![ReasonCode::GroupMemberLive]);

        *observer.census.lock().unwrap() = Some(InstanceCensus::Complete(Vec::new()));
        queue.enforce_deadlines(Instant::now() + Duration::from_secs(60));

        let snap2 = queue.collect_status_snapshot(Instant::now());
        assert!(snap2.held.is_empty());
        assert_eq!(snap2.recent_tasks[0].reference, "group-task");
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn each_unproven_observation_holds_and_its_changed_observation_releases() {
        // 1. RootLive -> Gone
        {
            let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observer = Arc::new(FakeTreeObserver::default());
            observer.verdicts.lock().unwrap().insert(
                1,
                InstanceVerdict::SameLive {
                    execution: ExecutionState::Running,
                },
            );
            let queue = queue_with_sink_and_observer(
                true,
                10,
                VecDeque::from([SpawnPlan::Process(FakeProcess::complete(1, cleanups))]),
                None,
                Arc::clone(&observer),
            );
            queue.submit(request("t-root"));
            queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
            assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);
            observer
                .verdicts
                .lock()
                .unwrap()
                .insert(1, InstanceVerdict::NotSameOrExited);
            queue.enforce_deadlines(Instant::now() + Duration::from_secs(60));
            assert!(
                queue
                    .collect_status_snapshot(Instant::now())
                    .held
                    .is_empty()
            );
        }
        // 2. BoundLive -> Gone
        {
            let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observer = Arc::new(FakeTreeObserver::default());
            observer.verdicts.lock().unwrap().insert(
                2,
                InstanceVerdict::SameLive {
                    execution: ExecutionState::Running,
                },
            );
            let mut fake = FakeProcess::complete(1, cleanups);
            let mut births = HashMap::new();
            births.insert(2, test_birth(200));
            fake.snapshot = Some(ProcessTreeSnapshot {
                parent_pid: 1,
                parent_pgid: Some(1),
                descendants: vec![Descendant {
                    pid: 2,
                    ppid: 1,
                    pgid: Some(1),
                    uid: current_task_uid(),
                }],
                descendant_births: births,
            });
            let queue = queue_with_sink_and_observer(
                true,
                10,
                VecDeque::from([SpawnPlan::Process(fake)]),
                None,
                Arc::clone(&observer),
            );
            queue.submit(request("t-bound"));
            queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
            assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);
            observer
                .verdicts
                .lock()
                .unwrap()
                .insert(2, InstanceVerdict::NotSameOrExited);
            queue.enforce_deadlines(Instant::now() + Duration::from_secs(60));
            assert!(
                queue
                    .collect_status_snapshot(Instant::now())
                    .held
                    .is_empty()
            );
        }
        // 3. Incomplete group -> Complete
        {
            let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observer = Arc::new(FakeTreeObserver::default());
            *observer.census.lock().unwrap() = Some(InstanceCensus::Incomplete(Vec::new()));
            let queue = queue_with_sink_and_observer(
                true,
                10,
                VecDeque::from([SpawnPlan::Process(FakeProcess::complete(1, cleanups))]),
                None,
                Arc::clone(&observer),
            );
            queue.submit(request("t-group"));
            queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();
            assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);
            *observer.census.lock().unwrap() = Some(InstanceCensus::Complete(Vec::new()));
            queue.enforce_deadlines(Instant::now() + Duration::from_secs(60));
            assert!(
                queue
                    .collect_status_snapshot(Instant::now())
                    .held
                    .is_empty()
            );
        }
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn recovery_signals_only_bound_identities() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            2,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        let mut fake = FakeProcess::complete(1, cleanups);
        let mut births = HashMap::new();
        births.insert(2, test_birth(200));
        fake.snapshot = Some(ProcessTreeSnapshot {
            parent_pid: 1,
            parent_pgid: Some(1),
            descendants: vec![Descendant {
                pid: 2,
                ppid: 1,
                pgid: Some(1),
                uid: current_task_uid(),
            }],
            descendant_births: births,
        });
        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(fake)]),
            None,
            Arc::clone(&observer),
        );
        queue.submit(request("t-sig"));
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();

        queue.enforce_deadlines(Instant::now());

        let signals = observer.signals.lock().unwrap().clone();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].0.pid, 2);
        assert_eq!(signals[0].1, SignalKind::Terminate);
    }

    #[test]
    fn live_root_is_terminated_through_a_snapshot_before_any_signal() {
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            1,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        let queue =
            queue_with_sink_and_observer(true, 10, VecDeque::new(), None, Arc::clone(&observer));
        let proc_handle: QueueProcessHandle = Arc::new(Mutex::new(Box::new(FakeProcess::idle(
            1,
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ))));
        {
            let mut state = queue.inner.state.lock().unwrap();
            state.held.insert(
                Partition::new("svc"),
                HeldEntry {
                    dispatch: dispatch("t-root-term"),
                    process: Some(proc_handle),
                    owner_uid: current_task_uid(),
                    bound_identities: Vec::new(),
                    bound_first_terminate_at: BTreeMap::new(),
                    first_held_at: Instant::now(),
                    first_held_at_unix: 0,
                    reasons: vec![ReasonCode::RootLive],
                    exit_code: -1,
                    termination_error: None,
                    snapshot_unavailable: false,
                    persisted: false,
                    records: Vec::new(),
                },
            );
        }
        queue.enforce_deadlines(Instant::now());
        assert!(observer.signals.lock().unwrap().is_empty());
    }

    #[test]
    fn reused_pids_never_become_bound_or_signalled() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        *observer.census.lock().unwrap() = Some(InstanceCensus::Complete(vec![
            CensusRow {
                instance: ProcessInstance {
                    pid: 1,
                    birth: test_birth(999),
                },
                uid: current_task_uid(),
                ppid: 0,
                pgid: 1,
                execution: ExecutionState::Running,
            },
            CensusRow {
                instance: ProcessInstance {
                    pid: 2,
                    birth: test_birth(200),
                },
                uid: 99999,
                ppid: 1,
                pgid: 1,
                execution: ExecutionState::Running,
            },
        ]));

        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(FakeProcess::complete(1, cleanups))]),
            None,
            Arc::clone(&observer),
        );
        queue.submit(request("reused"));
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();

        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
        assert_eq!(snap.recent_tasks.len(), 1);
        assert!(observer.signals.lock().unwrap().is_empty());
    }

    #[test]
    fn deadline_snapshot_is_bound_before_the_worker_can_prove() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            2,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        let mut fake = FakeProcess::idle(1, cleanups);
        let mut births = HashMap::new();
        births.insert(2, test_birth(200));
        fake.snapshot = Some(ProcessTreeSnapshot {
            parent_pid: 1,
            parent_pgid: Some(1),
            descendants: vec![Descendant {
                pid: 2,
                ppid: 1,
                pgid: Some(1),
                uid: current_task_uid(),
            }],
            descendant_births: births,
        });
        let proc_handle: QueueProcessHandle = Arc::new(Mutex::new(Box::new(fake)));
        let queue = queue_with_sink_and_observer(true, 10, VecDeque::new(), None, observer);
        add_active_process(&queue, "t-dl", proc_handle.clone());

        terminate_process(
            &queue.inner,
            "t-dl",
            0,
            proc_handle,
            Duration::from_millis(10),
        );

        let state = queue.inner.state.lock().unwrap();
        let active = state.active.get("t-dl").unwrap();
        assert_eq!(active.bound_identities.len(), 1);
        assert_eq!(active.bound_identities[0].instance.pid, 2);
    }

    #[test]
    fn deadline_termination_error_is_kept_and_surfaces_on_hold() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut fake = FakeProcess::idle(1, cleanups);
        fake.terminate_error = true;
        let proc_handle: QueueProcessHandle = Arc::new(Mutex::new(Box::new(fake)));
        let queue = queue(true, 10, VecDeque::new());
        add_active_process(&queue, "t-err", proc_handle.clone());

        terminate_process(
            &queue.inner,
            "t-err",
            0,
            proc_handle,
            Duration::from_millis(10),
        );

        let state = queue.inner.state.lock().unwrap();
        let active = state.active.get("t-err").unwrap();
        assert!(active.termination_error.is_some());
    }

    #[test]
    fn deadline_termination_thread_spawn_failure_is_recorded() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fake = FakeProcess::idle(1, cleanups);
        let proc_handle: QueueProcessHandle = Arc::new(Mutex::new(Box::new(fake)));
        let queue = queue(true, 10, VecDeque::new());
        queue
            .set_worker_thread_spawner(Arc::new(|_| Err(io::Error::other("injected spawn error"))));
        add_active_process(&queue, "t-spawn-fail", proc_handle.clone());

        start_termination(
            Arc::clone(&queue.inner),
            "t-spawn-fail".to_owned(),
            0,
            proc_handle,
            Duration::from_millis(10),
        );

        let state = queue.inner.state.lock().unwrap();
        let active = state.active.get("t-spawn-fail").unwrap();
        assert_eq!(
            active.termination_error.as_deref(),
            Some("failed to spawn termination thread")
        );
    }

    #[test]
    fn coalesced_references_get_one_held_and_one_stopped() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            1,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        let sink = Arc::new(RecordingEventSink::default());
        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(FakeProcess::complete(1, cleanups))]),
            Some(Arc::clone(&sink) as Arc<dyn TaskQueueEventSink>),
            observer,
        );
        queue.inner.state.lock().unwrap().running.insert(
            Partition::new("svc"),
            RunningSlot {
                reference: "primary".to_owned(),
                command: Vec::new(),
                background: false,
            },
        );
        assert_eq!(queue.submit(request("follower")), SubmitOutcome::Queued);
        assert_eq!(
            queue.submit(request("follower-2")),
            SubmitOutcome::Coalesced
        );

        let mut dispatch_multi = dispatch("primary");
        dispatch_multi.references.push("follower-2".to_owned());
        let events_before = sink.0.lock().unwrap().len();
        for reference in &dispatch_multi.references {
            emit_queue_event(
                &queue.inner.options.queue_sink,
                Some(TaskQueueEvent::Held {
                    partition: dispatch_multi.submission.partition.clone(),
                    reference: reference.clone(),
                    command: dispatch_multi.submission.command.clone(),
                    reasons: vec![ReasonCode::RootLive],
                }),
            );
        }
        let events = sink.0.lock().unwrap().clone();
        assert_eq!(events.len() - events_before, 2);
    }

    #[test]
    fn worker_panic_leaves_partition_held_and_tick_survives() {
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        let queue = queue_with_sink_and_observer(
            true,
            10,
            VecDeque::from([SpawnPlan::Process(FakeProcess::poll_panic(1, cleanups))]),
            None,
            observer,
        );

        assert_eq!(queue.submit(request("panicker")), SubmitOutcome::Dispatched);
        queue.join_test_workers(1, TEST_TRANSITION_TIMEOUT).unwrap();

        let snapshot = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snapshot.held.len(), 1);
        assert_eq!(
            snapshot.held[0].reasons,
            vec![ReasonCode::WorkerEndedWithoutProof]
        );
    }

    #[test]
    fn recovery_pass_panic_does_not_disable_recovery() {
        let queue = queue(true, 10, VecDeque::new());
        struct PanickingObserver;
        impl TreeObserver for PanickingObserver {
            fn census_group(&self, _pgid: i32, _deadline: Option<Instant>) -> InstanceCensus {
                panic!("injected observer panic")
            }
            fn observe(&self, _instance: &ProcessInstance) -> InstanceVerdict {
                panic!("injected observer panic")
            }
            fn process_owner(&self, _pid: u32) -> ProcessOwner {
                panic!("injected observer panic")
            }
            fn signal_exact(
                &self,
                _target: ProcessInstance,
                _signal: SignalKind,
            ) -> Result<(), TerminationError> {
                panic!("injected observer panic")
            }
            fn job_quiescent(&self) -> Result<bool, io::Error> {
                panic!("injected observer panic")
            }
            fn boot_identity(&self) -> Option<String> {
                panic!("injected observer panic")
            }
            fn supervisor_identity(&self) -> Option<ProcessInstance> {
                panic!("injected observer panic")
            }
            fn descendant_tree(
                &self,
                _root: &ProcessInstance,
                _owner_uid: u32,
            ) -> Result<Vec<ProcessInstance>, ()> {
                panic!("injected observer panic")
            }
        }
        queue.set_tree_observer(Arc::new(PanickingObserver));
        {
            let mut state = queue.inner.state.lock().unwrap();
            state.held.insert(
                Partition::new("svc"),
                HeldEntry {
                    dispatch: dispatch("held-panic"),
                    process: None,
                    owner_uid: current_task_uid(),
                    bound_identities: Vec::new(),
                    bound_first_terminate_at: BTreeMap::new(),
                    first_held_at: Instant::now(),
                    first_held_at_unix: 0,
                    reasons: vec![ReasonCode::RootLive],
                    exit_code: 0,
                    termination_error: None,
                    snapshot_unavailable: false,
                    persisted: false,
                    records: Vec::new(),
                },
            );
        }
        queue.enforce_deadlines(Instant::now());
        assert!(
            !queue
                .inner
                .recovery_in_progress
                .load(std::sync::atomic::Ordering::SeqCst)
        );

        queue.set_tree_observer(Arc::new(FakeTreeObserver::default()));
        queue.enforce_deadlines(Instant::now());
        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
    }

    #[test]
    fn recovery_never_waits_on_a_termination_thread() {
        let queue = queue(true, 10, VecDeque::new());
        let spawned_thread = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawned_flag = Arc::clone(&spawned_thread);
        queue.set_worker_thread_spawner(Arc::new(move |_| {
            spawned_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(thread::spawn(|| {
                thread::sleep(Duration::from_millis(50));
            }))
        }));
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fake = FakeProcess::idle(1, cleanups);
        let proc_handle: QueueProcessHandle = Arc::new(Mutex::new(Box::new(fake)));
        {
            let mut state = queue.inner.state.lock().unwrap();
            state.held.insert(
                Partition::new("svc"),
                HeldEntry {
                    dispatch: dispatch("held-async"),
                    process: Some(proc_handle),
                    owner_uid: current_task_uid(),
                    bound_identities: Vec::new(),
                    bound_first_terminate_at: BTreeMap::new(),
                    first_held_at: Instant::now(),
                    first_held_at_unix: 0,
                    reasons: vec![ReasonCode::RootLive],
                    exit_code: 0,
                    termination_error: None,
                    snapshot_unavailable: false,
                    persisted: false,
                    records: Vec::new(),
                },
            );
        }
        let start = Instant::now();
        queue.enforce_deadlines(Instant::now());
        assert!(start.elapsed() < Duration::from_millis(40));
    }

    #[test]
    fn status_and_submission_do_not_observe() {
        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: PathBuf::new(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
        assert_eq!(queue.collect_queue_counts(), BTreeMap::new());
    }

    #[test]
    fn shutdown_and_shutdown_until_hold_forced_and_termination() {
        let q1 = queue(true, 10, VecDeque::new());
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let proc_handle: QueueProcessHandle =
            Arc::new(Mutex::new(Box::new(FakeProcess::idle(1, cleanups))));
        {
            let mut state = q1.inner.state.lock().unwrap();
            state.held.insert(
                Partition::new("svc"),
                HeldEntry {
                    dispatch: dispatch("held-shutdown"),
                    process: Some(proc_handle),
                    owner_uid: current_task_uid(),
                    bound_identities: Vec::new(),
                    bound_first_terminate_at: BTreeMap::new(),
                    first_held_at: Instant::now(),
                    first_held_at_unix: 0,
                    reasons: vec![ReasonCode::RootLive],
                    exit_code: 0,
                    termination_error: None,
                    snapshot_unavailable: false,
                    persisted: false,
                    records: Vec::new(),
                },
            );
        }
        let report = q1.shutdown_with_timeout(Duration::from_millis(50));
        assert_eq!(report.active_count, 1);
        assert!(report.forced);

        let q2 = queue(true, 10, VecDeque::new());
        {
            let mut state = q2.inner.state.lock().unwrap();
            state.held.insert(
                Partition::new("svc"),
                HeldEntry {
                    dispatch: dispatch("held-shutdown-2"),
                    process: None,
                    owner_uid: current_task_uid(),
                    bound_identities: Vec::new(),
                    bound_first_terminate_at: BTreeMap::new(),
                    first_held_at: Instant::now(),
                    first_held_at_unix: 0,
                    reasons: vec![ReasonCode::RootLive],
                    exit_code: 0,
                    termination_error: None,
                    snapshot_unavailable: false,
                    persisted: false,
                    records: Vec::new(),
                },
            );
        }
        let report2 = q2.shutdown_until(Instant::now() + Duration::from_millis(100));
        assert_eq!(report2.active_count, 1);
        assert!(report2.forced);
    }

    #[test]
    fn shutdown_does_not_wait_out_its_deadline_for_a_live_hold() {
        for bounded in [false, true] {
            let queue = queue(true, 10, VecDeque::new());
            let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let process: QueueProcessHandle =
                Arc::new(Mutex::new(Box::new(FakeProcess::idle(1, cleanups))));
            queue.inner.state.lock().unwrap().held.insert(
                Partition::new("svc"),
                HeldEntry {
                    dispatch: dispatch("held-live-shutdown"),
                    process: Some(process),
                    owner_uid: current_task_uid(),
                    bound_identities: Vec::new(),
                    bound_first_terminate_at: BTreeMap::new(),
                    first_held_at: Instant::now(),
                    first_held_at_unix: 0,
                    reasons: vec![ReasonCode::RootLive],
                    exit_code: 0,
                    termination_error: None,
                    snapshot_unavailable: false,
                    persisted: false,
                    records: Vec::new(),
                },
            );
            let (sender, receiver) = std::sync::mpsc::channel();
            thread::spawn(move || {
                let report = if bounded {
                    queue.shutdown_until(Instant::now() + Duration::from_secs(60))
                } else {
                    queue.shutdown_with_timeout(Duration::from_secs(60))
                };
                let _ = sender.send(report);
            });
            let report = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("a held partition must not hold shutdown until its deadline");
            assert_eq!(report.active_count, 1);
            assert!(report.forced);
        }
    }

    #[test]
    fn intent_is_persisted_before_spawn() {
        let journal = tempfile::tempdir().expect("journal");
        let arrived = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);
        queue.set_worker_spawner(plan_spawner(VecDeque::from([SpawnPlan::Process(
            FakeProcess::gated(Arc::clone(&arrived), Arc::clone(&release), cleanups),
        )])));

        queue.submit(request("t-intent"));
        arrived.wait();

        let in_flight_dir = crate::queue_hold_store::in_flight_directory(journal.path());
        assert!(in_flight_dir.exists());
        let scopes = std::fs::read_dir(&in_flight_dir)
            .expect("read in-flight dir")
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(scopes.len(), 1);
        let records = std::fs::read_dir(scopes[0].path())
            .expect("read scope dir")
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        let rec = crate::queue_hold_store::read_in_flight_record(&records[0].path())
            .expect("read intent record");
        assert_eq!(rec.phase, "running");
        assert!(!rec.held);

        release.wait();
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("worker");
    }

    #[test]
    fn unwritable_records_hold_the_queue_without_spawning_or_history() {
        let journal = tempfile::tempdir().expect("journal");
        let observer = Arc::new(FakeTreeObserver::default());

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);
        queue.set_worker_spawner(plan_spawner(VecDeque::from([SpawnPlan::Panic(
            "must not spawn when record write fails",
        )])));

        let in_flight = crate::queue_hold_store::in_flight_directory(journal.path());
        std::fs::create_dir_all(in_flight.parent().unwrap()).unwrap();
        std::fs::write(&in_flight, b"blocking file").unwrap();

        assert_eq!(
            queue.submit(request("t-unwritable")),
            SubmitOutcome::Dispatched
        );
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("worker");

        let snap = queue.collect_status_snapshot(Instant::now());
        assert_eq!(
            snap.queue_hold.as_ref().map(|h| h.reason),
            Some(QueueHoldReason::RecordsUnavailable)
        );
        assert!(snap.recent_tasks.is_empty());
        assert_eq!(snap.queues.get("svc"), Some(&1));
    }

    #[test]
    fn record_is_removed_only_after_proof() {
        let journal = tempfile::tempdir().expect("journal");
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);
        queue.set_worker_spawner(plan_spawner(VecDeque::from([SpawnPlan::Process(
            FakeProcess::complete(1, cleanups),
        )])));

        queue.submit(request("t-proof"));
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("worker");

        let in_flight_dir = crate::queue_hold_store::in_flight_directory(journal.path());
        if in_flight_dir.exists() {
            for scope in std::fs::read_dir(&in_flight_dir).unwrap().flatten() {
                if scope.path().is_dir() {
                    let files = std::fs::read_dir(scope.path())
                        .unwrap()
                        .flatten()
                        .collect::<Vec<_>>();
                    assert!(files.is_empty());
                }
            }
        }
        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
        assert_eq!(snap.recent_tasks.len(), 1);
    }

    #[test]
    fn restart_loads_record_as_hold_and_queues_same_partition_work() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h1".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-held".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 10,
                birth: test_birth(10),
            }),
            group_id: Some(10),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });

        let snap = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snap.held.len(), 1);
        assert_eq!(snap.held[0].partition.as_str(), "svc");

        assert_eq!(queue.submit(request("t-next")), SubmitOutcome::Queued);
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn restart_hold_signals_only_recorded_bound_identities() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-bound".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-bound".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: vec![crate::queue_hold_store::PersistedBoundIdentity {
                pid: 200,
                birth: test_birth(200),
                uid: current_task_uid(),
            }],
            exit_code: Some(0),
            reasons: vec![ReasonCode::BoundLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());
        observer
            .verdicts
            .lock()
            .unwrap()
            .insert(100, InstanceVerdict::NotSameOrExited);
        observer.verdicts.lock().unwrap().insert(
            200,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());

        queue.enforce_deadlines(Instant::now());
        let signals = observer.signals.lock().unwrap().clone();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].0.pid, 200);
        assert_eq!(signals[0].1, SignalKind::Terminate);
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn unreadable_record_holds_across_two_restarts() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        std::fs::create_dir_all(rec_path.parent().unwrap()).unwrap();
        std::fs::write(&rec_path, b"invalid json content").unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());

        let q1 = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        q1.set_tree_observer(observer.clone());
        assert_eq!(q1.collect_status_snapshot(Instant::now()).held.len(), 1);
        q1.enforce_deadlines(Instant::now());
        assert_eq!(q1.collect_status_snapshot(Instant::now()).held.len(), 1);

        let q2 = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        q2.set_tree_observer(observer);
        assert_eq!(q2.collect_status_snapshot(Instant::now()).held.len(), 1);
    }

    #[test]
    fn record_from_an_earlier_boot_is_released_with_basis_reboot() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-boot".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-boot".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("new-boot-id".to_owned());

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());

        assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);
        queue.enforce_deadlines(Instant::now());
        assert!(
            queue
                .collect_status_snapshot(Instant::now())
                .held
                .is_empty()
        );
        assert!(observer.signals.lock().unwrap().is_empty());
        assert!(!rec_path.exists());
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn held_persist_and_recovery_release_do_not_resurrect_a_record() {
        let journal = tempfile::tempdir().expect("journal");
        let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            100,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());
        queue.set_worker_spawner(plan_spawner(VecDeque::from([SpawnPlan::Process(
            FakeProcess::poll_error(100, cleanups),
        )])));

        let queue_clone = queue.clone();
        queue.set_held_persist_hook(Some(Arc::new(move || {
            let snap = queue_clone.collect_status_snapshot(Instant::now());
            let _ = snap;
        })));

        queue.submit(request("t-resurrect"));
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("worker");

        assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);
    }

    #[test]
    fn unlistable_records_hold_the_whole_queue() {
        #[cfg(unix)]
        {
            if nix::unistd::getuid().is_root() {
                println!("skipping unlistable_records_hold_the_whole_queue: running as root");
                return;
            }
            use std::os::unix::fs::PermissionsExt;
            let journal = tempfile::tempdir().expect("journal");
            let in_flight = crate::queue_hold_store::in_flight_directory(journal.path());
            std::fs::create_dir_all(&in_flight).unwrap();
            std::fs::set_permissions(&in_flight, std::fs::Permissions::from_mode(0o000)).unwrap();

            let queue = TaskQueue::new(TaskQueueOptions {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                journal_root: journal.path().to_path_buf(),
                cap_resolver: Arc::new(FixedCap(10)),
                process_state_probe: Arc::new(UnreachableProcessStateProbe),
                queue_sink: None,
                process_sink: None,
                ready: true,
                before_deadline_commit: None,
                child_environment: BTreeMap::new(),
                task_binary: None,
            });

            let snap = queue.collect_status_snapshot(Instant::now());
            assert_eq!(
                snap.queue_hold.as_ref().map(|h| h.reason),
                Some(QueueHoldReason::RecordsUnreadable)
            );
            std::fs::set_permissions(&in_flight, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn newly_listable_directory_is_loaded_before_clearing() {
        let journal = tempfile::tempdir().expect("journal");
        let observer = Arc::new(FakeTreeObserver::default());
        observer.verdicts.lock().unwrap().insert(
            10,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);

        {
            let mut state = queue.inner.state.lock().unwrap();
            state.queue_hold = Some(QueueHoldStatus {
                reason: QueueHoldReason::RecordsUnreadable,
                detail: "simulated unlistable".to_owned(),
            });
        }

        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-p1".to_owned(),
            partition: "p1".to_owned(),
            references: vec!["t-p1".to_owned()],
            command: vec!["p1".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 10,
                birth: test_birth(10),
            }),
            group_id: Some(10),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("p1"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        queue.enforce_deadlines(Instant::now());
        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.queue_hold.is_none());
        assert_eq!(snap.held.len(), 1);
        assert_eq!(snap.held[0].partition.as_str(), "p1");
    }

    #[test]
    fn identity_unavailable_hold_does_not_flap() {
        let journal = tempfile::tempdir().expect("journal");
        let observer = Arc::new(FakeTreeObserver::default());
        *observer.supervisor_id.lock().unwrap() = None;

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);

        queue.submit(request("t-flap"));
        queue
            .join_test_workers(1, TEST_TRANSITION_TIMEOUT)
            .expect("worker");

        assert_eq!(
            queue
                .collect_status_snapshot(Instant::now())
                .queue_hold
                .as_ref()
                .map(|h| h.reason),
            Some(QueueHoldReason::RecordsUnavailable)
        );

        queue.enforce_deadlines(Instant::now());
        assert_eq!(
            queue
                .collect_status_snapshot(Instant::now())
                .queue_hold
                .as_ref()
                .map(|h| h.reason),
            Some(QueueHoldReason::RecordsUnavailable)
        );
    }

    #[test]
    fn debris_is_not_a_record() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let scope_dir = crate::queue_hold_store::in_flight_directory(journal.path()).join(scope);
        std::fs::create_dir_all(&scope_dir).unwrap();
        std::fs::write(scope_dir.join(".DS_Store"), b"junk").unwrap();
        std::fs::write(scope_dir.join(".tmp_hold_probe_1.tmp"), b"junk").unwrap();
        std::fs::write(scope_dir.join("other.txt"), b"junk").unwrap();
        std::fs::create_dir_all(scope_dir.join("subfolder")).unwrap();

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });

        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
        assert!(snap.queue_hold.is_none());
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn two_records_for_one_partition_hold_until_both_are_proven() {
        let journal = tempfile::tempdir().expect("journal");
        let scope1 = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let scope2 = "05060708_p00000002_b00000000000000020000000000000064";

        let rec1 = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h1".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["ref-1".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: 100,
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec2 = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h2".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["ref-2".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: 200,
            root: Some(ProcessInstance {
                pid: 200,
                birth: test_birth(200),
            }),
            group_id: Some(200),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };

        let p1 = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope1,
            &Partition::new("svc"),
        );
        let p2 = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope2,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&p1, &rec1).unwrap();
        crate::queue_hold_store::write_in_flight_record(&p2, &rec2).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());
        observer
            .verdicts
            .lock()
            .unwrap()
            .insert(100, InstanceVerdict::NotSameOrExited);
        observer.verdicts.lock().unwrap().insert(
            200,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());

        assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);

        queue.enforce_deadlines(Instant::now());
        assert!(!p1.exists());
        assert!(p2.exists());
        assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);

        observer
            .verdicts
            .lock()
            .unwrap()
            .insert(200, InstanceVerdict::NotSameOrExited);
        queue.enforce_deadlines(Instant::now());
        assert!(!p2.exists());
        assert!(
            queue
                .collect_status_snapshot(Instant::now())
                .held
                .is_empty()
        );
    }

    #[test]
    fn restarted_hold_releases_one_stopped_per_recorded_reference() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-multi-ref".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["ref-a".to_owned(), "ref-b".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: Vec::new(),
            exit_code: Some(42),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        struct EventRecorder(Mutex<Vec<TaskQueueEvent>>);
        impl TaskQueueEventSink for EventRecorder {
            fn emit(&self, event: TaskQueueEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let sink = Arc::new(EventRecorder(Mutex::new(Vec::new())));

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());
        observer
            .verdicts
            .lock()
            .unwrap()
            .insert(100, InstanceVerdict::NotSameOrExited);

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: Some(sink.clone()),
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);

        queue.enforce_deadlines(Instant::now());
        let events = sink.0.lock().unwrap().clone();
        let stopped_refs = events
            .iter()
            .filter_map(|e| match e {
                TaskQueueEvent::Stopped {
                    reference,
                    exit_code,
                    ..
                } => Some((reference.clone(), *exit_code)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            stopped_refs,
            vec![("ref-a".to_owned(), 42), ("ref-b".to_owned(), 42)]
        );
    }

    #[test]
    fn exited_before_identity_record_releases_on_observation() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-exit".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-exit".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());
        observer
            .verdicts
            .lock()
            .unwrap()
            .insert(100, InstanceVerdict::NotSameOrExited);

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);

        queue.enforce_deadlines(Instant::now());
        assert!(
            queue
                .collect_status_snapshot(Instant::now())
                .held
                .is_empty()
        );
        assert!(!rec_path.exists());
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn termination_bound_identities_survive_a_restart() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-term-survive".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-term-survive".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: vec![crate::queue_hold_store::PersistedBoundIdentity {
                pid: 105,
                birth: test_birth(105),
                uid: current_task_uid(),
            }],
            exit_code: Some(0),
            reasons: vec![ReasonCode::BoundLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());
        observer
            .verdicts
            .lock()
            .unwrap()
            .insert(100, InstanceVerdict::NotSameOrExited);
        observer.verdicts.lock().unwrap().insert(
            105,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());

        let t0 = Instant::now();
        queue.enforce_deadlines(t0);
        assert_eq!(observer.signals.lock().unwrap().len(), 1);
        assert_eq!(observer.signals.lock().unwrap()[0].1, SignalKind::Terminate);

        queue.enforce_deadlines(t0 + CAP_TERMINATION_TIMEOUT + Duration::from_millis(10));
        assert_eq!(observer.signals.lock().unwrap().len(), 2);
        assert_eq!(observer.signals.lock().unwrap()[1].1, SignalKind::Kill);
    }

    #[test]
    fn failed_rewrite_is_visible_and_retried() {
        let journal = tempfile::tempdir().expect("journal");
        let observer = Arc::new(FakeTreeObserver::default());

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer);

        queue.enforce_deadlines(Instant::now());
        let snap = queue.collect_status_snapshot(Instant::now());
        assert!(snap.held.is_empty());
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn handle_less_live_root_is_snapshotted_before_it_is_signalled() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-snap-first".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-snap-first".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 100,
                birth: test_birth(100),
            }),
            group_id: Some(100),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::RootLive],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());
        observer.verdicts.lock().unwrap().insert(
            100,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        observer.verdicts.lock().unwrap().insert(
            101,
            InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            },
        );
        observer.descendant_trees.lock().unwrap().insert(
            100,
            Ok(vec![ProcessInstance {
                pid: 101,
                birth: test_birth(101),
            }]),
        );

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());

        queue.enforce_deadlines(Instant::now());
        let signals = observer.signals.lock().unwrap().clone();
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].0.pid, 100);
        assert_eq!(signals[0].1, SignalKind::Terminate);
        assert_eq!(signals[1].0.pid, 101);
        assert_eq!(signals[1].1, SignalKind::Terminate);
    }

    // Unix PID/group recovery fixture; Windows proves liveness through supervisor/jobs.
    #[cfg(unix)]
    #[test]
    fn intent_without_root_holds_until_reboot() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "intent".to_owned(),
            hold_id: "h-intent-noroot".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-intent-noroot".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: None,
            group_id: None,
            bound: Vec::new(),
            exit_code: None,
            reasons: Vec::new(),
            termination_error: None,
            snapshot_unavailable: false,
            held: false,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let observer = Arc::new(FakeTreeObserver::default());
        *observer.boot_id.lock().unwrap() = Some("boot".to_owned());

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });
        queue.set_tree_observer(observer.clone());

        let snap = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snap.held.len(), 1);
        assert_eq!(snap.held[0].reasons, vec![ReasonCode::RootUnknown]);

        queue.enforce_deadlines(Instant::now());
        assert_eq!(queue.collect_status_snapshot(Instant::now()).held.len(), 1);

        *observer.boot_id.lock().unwrap() = Some("boot-after-reboot".to_owned());
        queue.enforce_deadlines(Instant::now());
        assert!(
            queue
                .collect_status_snapshot(Instant::now())
                .held
                .is_empty()
        );
        assert!(!rec_path.exists());
    }

    #[test]
    fn new_and_collect_status_snapshot_on_recorded_journal_do_not_panic() {
        let journal = tempfile::tempdir().expect("journal");
        let scope = &crate::queue_hold_store::format_scope_dir_name(
            Some("boot"),
            &ProcessInstance {
                pid: 1,
                birth: ProcessBirth::linux(1, 0, 100),
            },
        );
        let record = crate::queue_hold_store::InFlightRecord {
            phase: "running".to_owned(),
            hold_id: "h-nopanic".to_owned(),
            partition: "svc".to_owned(),
            references: vec!["t-nopanic".to_owned()],
            command: vec!["svc".to_owned()],
            day: None,
            scheduler_name: None,
            uid: current_task_uid(),
            created_unix: unix_seconds(),
            root: Some(ProcessInstance {
                pid: 10,
                birth: test_birth(10),
            }),
            group_id: Some(10),
            bound: Vec::new(),
            exit_code: Some(0),
            reasons: vec![ReasonCode::UnprovenAtStart],
            termination_error: None,
            snapshot_unavailable: false,
            held: true,
        };
        let rec_path = crate::queue_hold_store::partition_record_path(
            journal.path(),
            scope,
            &Partition::new("svc"),
        );
        crate::queue_hold_store::write_in_flight_record(&rec_path, &record).unwrap();

        let queue = TaskQueue::new(TaskQueueOptions {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            journal_root: journal.path().to_path_buf(),
            cap_resolver: Arc::new(FixedCap(10)),
            process_state_probe: Arc::new(UnreachableProcessStateProbe),
            queue_sink: None,
            process_sink: None,
            ready: true,
            before_deadline_commit: None,
            child_environment: BTreeMap::new(),
            task_binary: None,
        });

        let snap = queue.collect_status_snapshot(Instant::now());
        assert_eq!(snap.held.len(), 1);
        assert_eq!(snap.held[0].partition.as_str(), "svc");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_worker_preserves_unavailable_policy_and_runs_available_twin() {
        use solstone_core_journal_config::work_priority::WorkBand;

        let root = tempfile::tempdir().unwrap();
        let (sent, received) = mpsc::channel();
        let callable = Arc::new(move |command: &[String], _control: CallableTaskContext| {
            sent.send(command.to_vec()).unwrap();
            0
        });
        let queue = TaskQueue::with_callable_worker(
            TaskQueueOptions {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                journal_root: root.path().to_path_buf(),
                cap_resolver: Arc::new(FixedCap(10)),
                process_state_probe: Arc::new(UnreachableProcessStateProbe),
                queue_sink: None,
                process_sink: None,
                ready: true,
                before_deadline_commit: None,
                child_environment: BTreeMap::new(),
                task_binary: None,
            },
            callable,
        );
        assert_eq!(
            read_work_policy(&queue.inner).lane,
            ThinkingLane::Unavailable
        );

        let command = vec![
            "solstone".to_owned(),
            "journal".to_owned(),
            "think".to_owned(),
            "--day".to_owned(),
            "20261012".to_owned(),
        ];
        let submission = Submission {
            discovered: false,
            partition: Partition::new("think"),
            cap: Duration::from_secs(60),
            command: command.clone(),
            reference: "analysis-test-ref".to_owned(),
            day: Some("20261012".to_owned()),
            scheduler_name: None,
            daily_catchup_provenance: None,
            priority: PriorityIntent::default(),
        };
        let dispatch = Dispatch {
            priority: WorkPriority {
                band: WorkBand::Recent,
                source_time: 20261012000000,
            },
            sources: vec![submission.clone()],
            submission,
            references: vec!["analysis-test-ref".to_owned()],
            daily_catchup_admission: None,
        };

        start_dispatch(Arc::clone(&queue.inner), dispatch);
        assert!(received.recv_timeout(Duration::from_millis(30)).is_err());
        assert!(queue.contains_reference("analysis-test-ref"));
        *queue.inner.work_policy_override.lock().unwrap() = Some(local_spending());
        queue.resume_processing();
        let dispatched = received
            .recv_timeout(Duration::from_secs(5))
            .expect("command should be dispatched to callable worker");
        assert_eq!(dispatched, command);
        let _ = queue.shutdown();
        queue.join_callable_workers().unwrap();
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_deadline_shutdown_retains_the_real_worker_until_join() {
        let root = tempfile::tempdir().unwrap();
        let (entered_send, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let worker: CallableQueueWorker = Arc::new(move |_, stop| {
            entered_send.send(stop.cancelled).unwrap();
            released.lock().unwrap().recv().unwrap();
            0
        });
        let queue = TaskQueue::with_callable_worker(
            TaskQueueOptions {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                journal_root: root.path().to_path_buf(),
                cap_resolver: Arc::new(FixedCap(10)),
                process_state_probe: Arc::new(UnreachableProcessStateProbe),
                queue_sink: None,
                process_sink: None,
                ready: true,
                before_deadline_commit: None,
                child_environment: BTreeMap::new(),
                task_binary: None,
            },
            worker,
        );
        queue.submit(request("callable-held"));
        let stop = entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        let report = queue.shutdown_until(Instant::now() + Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(report.forced);
        assert!(stop.load(std::sync::atomic::Ordering::Acquire));
        {
            let workers = queue.inner.callable_workers.lock().unwrap();
            let entry = workers
                .iter()
                .find(|entry| entry.reference == "callable-held")
                .unwrap();
            assert!(
                !entry.joined.load(std::sync::atomic::Ordering::Acquire)
                    && entry.observer.is_some(),
                "pending shutdown must retain the observer owning the actual native join"
            );
        }
        release.send(()).unwrap();
        queue.join_callable_workers().unwrap();
        assert!(queue.inner.callable_workers.lock().unwrap().is_empty());
        assert!(!queue.contains_reference("callable-held"));
    }

    #[test]
    fn callable_admission_refuses_held_daily_before_catchup_attempt() {
        let root = tempfile::tempdir().unwrap();
        let queue = TaskQueue::with_callable_worker_and_admission(
            TaskQueueOptions {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                journal_root: root.path().to_path_buf(),
                cap_resolver: Arc::new(FixedCap(10)),
                process_state_probe: Arc::new(UnreachableProcessStateProbe),
                queue_sink: None,
                process_sink: None,
                ready: true,
                before_deadline_commit: None,
                child_environment: BTreeMap::new(),
                task_binary: None,
            },
            Arc::new(|_, _| panic!("held work must not execute")),
            Arc::new(|_| false),
            None,
        );
        let outcome = queue.submit(command_request(
            &[
                "solstone",
                "journal",
                "think",
                "--day",
                "20261012",
                "--daily-only",
            ],
            "held-daily",
            Some(DailyCatchupProvenance {
                day: "20261012".into(),
            }),
        ));
        assert!(matches!(outcome, SubmitOutcome::Rejected));
        assert!(!queue.contains_reference("held-daily"));
        assert!(!root.path().join("health/catchup-state.json").exists());
        let _ = queue.shutdown();
        queue.join_callable_workers().unwrap();
    }

    #[cfg(feature = "full-tests")]
    fn callable_control_queue(
        worker: CallableQueueWorker,
        cap: Duration,
        sink: Option<Arc<dyn TaskQueueEventSink>>,
    ) -> TaskQueue {
        struct TestCap(Duration);
        impl CapResolver for TestCap {
            fn cap_for(&self, _: &Partition) -> Duration {
                self.0
            }
        }
        let queue = TaskQueue::with_callable_worker(
            TaskQueueOptions {
                #[cfg(windows)]
                read_file_grants: Vec::new(),
                journal_root: PathBuf::new(),
                cap_resolver: Arc::new(TestCap(cap)),
                process_state_probe: Arc::new(UnreachableProcessStateProbe),
                queue_sink: sink,
                process_sink: None,
                ready: true,
                before_deadline_commit: None,
                child_environment: BTreeMap::new(),
                task_binary: None,
            },
            worker,
        );
        *queue.inner.work_policy_override.lock().unwrap() = Some(local_spending());
        queue
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_context_preserves_explicit_priority_and_expires_at_the_task_cap() {
        use solstone_core_journal_config::work_priority::WorkBand;
        let (send, received) = mpsc::channel();
        let worker: CallableQueueWorker = Arc::new(move |_, context| {
            thread::sleep(
                context.deadline.saturating_duration_since(Instant::now())
                    + Duration::from_millis(5),
            );
            send.send((context.priority, context.is_cancelled()))
                .unwrap();
            0
        });
        let sink = Arc::new(RecordingEventSink(Mutex::new(Vec::new())));
        let queue = callable_control_queue(worker, Duration::from_millis(50), Some(sink.clone()));
        let expected = WorkPriority {
            band: WorkBand::Import,
            source_time: 20200101120000,
        };
        let mut request = request("callable-cap");
        let ExecutionRequest::Bus(bus) = &mut request else {
            unreachable!()
        };
        bus.priority = Some(expected);
        let began = Instant::now();
        queue.submit(request);
        let (priority, cancelled) = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(priority, expected);
        assert!(
            cancelled,
            "the actual task cap must reach the callable checkpoint"
        );
        assert!(began.elapsed() < Duration::from_secs(1));
        queue.join_callable_workers().unwrap();
        assert!(sink.0.lock().unwrap().iter().any(|event| matches!(event,
            TaskQueueEvent::Stopped { reference, exit_code: -1, .. } if reference == "callable-cap")));
    }

    #[cfg(feature = "full-tests")]
    #[derive(Clone, Default)]
    struct CallableRetirementGate(Arc<(Mutex<bool>, Condvar)>);

    #[cfg(feature = "full-tests")]
    impl CallableRetirementGate {
        fn wait(&self) {
            let (lock, ready) = &*self.0;
            let mut released = lock.lock().unwrap_or_else(|p| p.into_inner());
            let deadline = Instant::now() + Duration::from_secs(10);
            while !*released {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return;
                }
                released = ready
                    .wait_timeout(released, remaining)
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
        }

        fn release(&self) {
            *self.0.0.lock().unwrap_or_else(|p| p.into_inner()) = true;
            self.0.1.notify_all();
        }
    }

    #[cfg(feature = "full-tests")]
    struct ReleaseCallableRetirement(CallableRetirementGate);

    #[cfg(feature = "full-tests")]
    impl Drop for ReleaseCallableRetirement {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_reference_reuse_retains_both_handles_until_observers_join() {
        struct Completed(mpsc::Sender<()>);
        impl TaskQueueEventSink for Completed {
            fn emit(&self, event: TaskQueueEvent) {
                if matches!(
                    event,
                    TaskQueueEvent::QueueChanged {
                        running_reference: None,
                        ..
                    }
                ) {
                    let _ = self.0.send(());
                }
            }
        }
        let (completed_send, completed) = mpsc::channel();
        let queue = callable_control_queue(
            Arc::new(|_, _| 0),
            Duration::from_secs(30),
            Some(Arc::new(Completed(completed_send))),
        );
        let gate = CallableRetirementGate::default();
        let release = ReleaseCallableRetirement(gate.clone());
        let (entered_send, entered) = mpsc::channel();
        // Gate observers outside TLS: Windows holds the loader lock in TLS
        // destructors, so a blocked destructor cannot await a new thread.
        queue.set_worker_thread_spawner(Arc::new(move |join| {
            let gate = gate.clone();
            let entered_send = entered_send.clone();
            thread::Builder::new().spawn(move || {
                let _ = entered_send.send(());
                gate.wait();
                join();
            })
        }));
        queue.submit(request("reused-reference"));
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            !queue.contains_reference("reused-reference"),
            "logical completion precedes native join"
        );
        queue.submit(request("reused-reference"));
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let retained = {
            let workers = queue.inner.callable_workers.lock().unwrap();
            workers
                .iter()
                .filter(|entry| !entry.joined.load(std::sync::atomic::Ordering::Acquire))
                .count()
        };
        let began = Instant::now();
        let report = queue.shutdown_until(Instant::now() + Duration::from_millis(20));
        let elapsed = began.elapsed();
        drop(release);
        queue.join_callable_workers().unwrap();
        assert!(
            elapsed < Duration::from_millis(500),
            "pending native joins must not block bounded shutdown: {elapsed:?}"
        );
        assert_eq!(
            retained, 2,
            "a reused reference must not replace the first live handle"
        );
        assert!(report.forced, "unjoined workers are real pending work");
        assert!(queue.inner.callable_workers.lock().unwrap().is_empty());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_tls_retirement_retains_native_join_and_bounds_shutdown() {
        struct ExitWait {
            entered: mpsc::Sender<()>,
            gate: CallableRetirementGate,
        }
        impl Drop for ExitWait {
            fn drop(&mut self) {
                if self.entered.send(()).is_ok() {
                    self.gate.wait();
                }
            }
        }
        thread_local! { static EXIT_WAIT: std::cell::RefCell<Option<ExitWait>> = const { std::cell::RefCell::new(None) }; }
        let gate = CallableRetirementGate::default();
        let body_gate = CallableRetirementGate::default();
        let (entered_send, entered) = mpsc::channel();
        let release = ReleaseCallableRetirement(gate.clone());
        let body_release = ReleaseCallableRetirement(body_gate.clone());
        let queue = callable_control_queue(
            Arc::new(move |_, _| {
                EXIT_WAIT.with(|slot| {
                    *slot.borrow_mut() = Some(ExitWait {
                        entered: entered_send.clone(),
                        gate: gate.clone(),
                    })
                });
                body_gate.wait();
                0
            }),
            Duration::from_secs(30),
            None,
        );
        let (observer_send, observer_started) = mpsc::channel();
        queue.set_worker_thread_spawner(Arc::new(move |join| {
            let observer_send = observer_send.clone();
            thread::Builder::new().spawn(move || {
                let _ = observer_send.send(());
                join();
            })
        }));
        queue.submit(request("tls-retirement"));
        // Start the actual native-join owner before entering the Windows TLS
        // loader lock. No second worker or observer is spawned while it is held.
        observer_started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        drop(body_release);
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!queue.contains_reference("tls-retirement"));
        {
            let workers = queue.inner.callable_workers.lock().unwrap();
            assert_eq!(workers.len(), 1);
            assert!(workers[0].observer.is_some());
            assert!(!workers[0].joined.load(std::sync::atomic::Ordering::Acquire));
        }
        let began = Instant::now();
        let report = queue.shutdown_until(Instant::now() + Duration::from_millis(20));
        let elapsed = began.elapsed();
        drop(release);
        queue.join_callable_workers().unwrap();
        assert!(
            elapsed < Duration::from_millis(500),
            "TLS teardown must not block bounded shutdown: {elapsed:?}"
        );
        assert!(report.forced, "TLS retirement is real pending work");
        assert!(queue.inner.callable_workers.lock().unwrap().is_empty());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_observer_spawn_failure_retains_the_original_worker_for_retirement() {
        let (entered_send, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        let queue = callable_control_queue(
            Arc::new(move |_, _| {
                entered_send.send(()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                0
            }),
            Duration::from_secs(30),
            None,
        );
        queue.set_worker_thread_spawner(Arc::new(|_| {
            Err(io::Error::other("join observer spawn refused"))
        }));
        queue.submit(request("observer-failure"));
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        let began = Instant::now();
        let report = queue.shutdown_until(Instant::now() + Duration::from_millis(20));
        assert!(began.elapsed() < Duration::from_millis(500));
        assert!(report.forced);
        {
            let workers = queue.inner.callable_workers.lock().unwrap();
            assert_eq!(workers.len(), 1);
            assert!(workers[0].observer.is_none());
            assert!(workers[0].worker.lock().unwrap().is_some());
            assert!(!workers[0].joined.load(std::sync::atomic::Ordering::Acquire));
        }
        release.send(()).unwrap();
        assert!(
            queue.join_callable_workers().is_err(),
            "observer failure stays failed after real drain"
        );
        assert!(queue.inner.callable_workers.lock().unwrap().is_empty());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn callable_panic_payload_is_retained_and_failure_stays_sticky() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Payload(Arc<AtomicBool>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let payload_flag = Arc::clone(&dropped);
        let queue = callable_control_queue(
            Arc::new(move |_, _| std::panic::panic_any(Payload(Arc::clone(&payload_flag)))),
            Duration::from_secs(30),
            None,
        );
        queue.submit(request("callable-panic"));
        assert!(queue.join_callable_workers().is_err());
        assert!(queue.callable_workers_failed());
        assert_eq!(queue.inner.callable_panics.lock().unwrap().len(), 1);
        assert!(!dropped.load(Ordering::Acquire));
        assert!(queue.join_callable_workers().is_err());
    }
}
