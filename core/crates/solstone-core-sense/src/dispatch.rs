// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value, json};
use solstone_core_journal_config::work_priority::{WORK_PRIORITY_ENV, WorkBand, WorkPriority};
use solstone_core_system::lifecycle::HostedServiceParentRuntime;
use solstone_core_system::process::{
    ChildLaunchContext, Disposition, ManagedLaunchRequest, ManagedProcess,
    SERVICE_SHUTDOWN_TIMEOUT, SpawnOptions, TerminationError, launch_managed_hosted,
    launch_managed_request,
};

use crate::beacon::Health;
use crate::config::{
    HANDLERS, no_thinking_engine, processing_deferred, read_config, resolve_concurrency,
    resolve_max_runtime,
};
use crate::events;
use crate::log;
use crate::memory::{Admission, SystemMemoryProbe};
use crate::registry::{
    DispatcherResolveError, HandlerSpec, command_for, default_registry, match_handler,
    resolve_dispatcher_in, segment_dir,
};
use crate::work::{SegmentContext, SegmentKey, SegmentState, WorkItem};

const PROVIDER_BLOCKED: i32 = 69;
static HOSTED_CHILD_LAUNCH_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// How long the live path keeps retrying a same-day admission that lost the
/// sense-day lease to a repair batch, before giving up and reporting it.
///
/// Mirrors `solstone-core-think-cli`'s `SENSE_PHASE_TIMEOUT` (1800s): the
/// system already tolerates a same-day `sense_batch` run holding this lease
/// for that long, so the live retry window matches rather than inventing a
/// second, smaller contract the repair batch can still outlast.
const LIVE_DAY_LEASE_RETRY_BUDGET: Duration = Duration::from_secs(1_800);

#[derive(Debug, Clone)]
pub struct Outbound {
    pub tract: &'static str,
    pub event: &'static str,
    pub fields: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SenseJobResult {
    Finished(i32),
    Hold,
}

pub trait SenseExecutor: Send + Sync {
    fn cancelled(&self) -> bool {
        false
    }
    fn execute(
        &self,
        handler: &str,
        path: &std::path::Path,
        context: &crate::work::SegmentContext,
        attempt: Option<&solstone_core_system::work_retry::Attempt>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> SenseJobResult;
}

struct Job {
    item: WorkItem,
    spec: HandlerSpec,
    config: Map<String, Value>,
    priority: WorkPriority,
}
struct State {
    segments: HashMap<SegmentKey, SegmentState>,
    pending_files: HashSet<(String, PathBuf)>,
    health: Health,
    stopping: bool,
    day_leases: HashMap<String, solstone_core_journal_io::FileLease>,
    lease_deferred: Vec<LeaseDeferredAdmission>,
}

/// A live segment whose day-lease acquisition lost to a same-day repair
/// batch. Retried from [`SenseDispatcher::retry_lease_deferred`] on every
/// status tick until the lease frees up or `LIVE_DAY_LEASE_RETRY_BUDGET`
/// elapses.
struct LeaseDeferredAdmission {
    context: SegmentContext,
    paths: Vec<PathBuf>,
    first_deferred_at: Instant,
}

/// Whether a completed Sense batch owns a stream-dirty transition.
///
/// This is deliberately separate from the batch event presentation flag: a
/// standalone historical batch changes day content and must dirty the day,
/// while the enclosing whole-day lifecycle publishes that transition itself.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BatchMarkerPolicy {
    #[default]
    AdvanceStream,
    EnclosedWholeDay,
}

/// Batch-only settings shared by dispatcher construction and native children.
#[derive(Clone, Default)]
struct BatchContext {
    describe_workers: Option<usize>,
    child_environment: solstone_core_system::process::ChildLaunchContext,
    marker_policy: BatchMarkerPolicy,
}

/// Per-worker state that remains fixed for its lifetime.
#[derive(Clone)]
struct WorkerContext {
    journal: PathBuf,
    admission: Admission,
    verbose: bool,
    debug: bool,
    batch: BatchContext,
    program: Result<PathBuf, DispatcherResolveError>,
    tally: Arc<JobTally>,
    cleanups: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
    executor: Option<Arc<dyn SenseExecutor>>,
}

/// Per-run handler outcome counts. Incremented as `run_job` finishes, then
/// read after the dispatcher is idle. Not a rolling beacon.
pub(crate) struct JobTally {
    ran: AtomicUsize,
    failed: AtomicUsize,
    deferred: AtomicUsize,
}

impl JobTally {
    fn new() -> Self {
        Self {
            ran: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            deferred: AtomicUsize::new(0),
        }
    }

    fn success(&self) {
        self.ran.fetch_add(1, Ordering::SeqCst);
    }

    fn failure(&self) {
        self.ran.fetch_add(1, Ordering::SeqCst);
        self.failed.fetch_add(1, Ordering::SeqCst);
    }

    fn defer(&self) {
        self.ran.fetch_add(1, Ordering::SeqCst);
        self.deferred.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn deferred(&self) -> usize {
        self.deferred.load(Ordering::SeqCst)
    }

    pub(crate) fn snapshot(&self) -> (usize, usize) {
        (
            self.failed.load(Ordering::SeqCst),
            self.ran.load(Ordering::SeqCst),
        )
    }
}

pub struct SenseDispatcher {
    archive_admission: Mutex<()>,
    journal: PathBuf,
    state: Arc<Mutex<State>>,
    outbound: mpsc::Sender<Outbound>,
    queues: HashMap<&'static str, Arc<Mutex<HandlerQueue>>>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    cleanups: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    admission: Admission,
    batch: BatchContext,
    pub(crate) tally: Arc<JobTally>,
    executor: Option<Arc<dyn SenseExecutor>>,
}

fn archive_pending_files_in(state: &State) -> usize {
    state
        .segments
        .values()
        .filter(|segment| is_archive(&segment.context))
        .map(|segment| segment.pending.len())
        .sum::<usize>()
        + state
            .lease_deferred
            .iter()
            .filter(|pending| is_archive(&pending.context))
            .map(|pending| pending.paths.len())
            .sum::<usize>()
}

fn is_archive(context: &SegmentContext) -> bool {
    context
        .meta
        .as_ref()
        .and_then(|m| m.get("audio_archive"))
        .and_then(Value::as_bool)
        == Some(true)
}

impl SenseDispatcher {
    pub fn new(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
    ) -> Self {
        Self::new_with_hosted_parent(
            journal,
            verbose,
            debug,
            outbound,
            solstone_core_system::process::ChildLaunchContext::default(),
            None,
        )
    }

    /// Construct a dispatcher whose service-like children participate in the
    /// hosted generation that admitted this Sense service.
    ///
    /// `child_environment` carries the inherited speakers-analyze generation
    /// (see `solstone-core-transcribe::SpeakersAnalyzeGeneration`) so native
    /// transcribe children spawned by this dispatcher borrow the hosted
    /// owner's generation instead of each attempting their own acquisition.
    pub fn new_with_hosted_parent(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        child_environment: solstone_core_system::process::ChildLaunchContext,
        hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
    ) -> Self {
        Self::new_with_admission(
            journal,
            verbose,
            debug,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            child_environment,
            hosted_parent,
        )
    }

    pub fn new_with_admission(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        admission: Admission,
        child_environment: solstone_core_system::process::ChildLaunchContext,
        hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
    ) -> Self {
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            admission,
            resolve_program_from_current_exe(),
            BatchContext {
                child_environment,
                ..BatchContext::default()
            },
            hosted_parent,
            None,
        )
    }

    pub fn new_with_executor(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        child_environment: solstone_core_system::process::ChildLaunchContext,
        hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
        executor: Arc<dyn SenseExecutor>,
    ) -> Self {
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            resolve_program_from_current_exe(),
            BatchContext {
                child_environment,
                ..BatchContext::default()
            },
            hosted_parent,
            Some(executor),
        )
    }

    pub fn new_with_fixture_program_and_admission(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        program: PathBuf,
        admission: Admission,
    ) -> Self {
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            admission,
            Ok(program),
            BatchContext::default(),
            None,
            None,
        )
    }

    /// Construct a dispatcher that uses a built fixture executable. This keeps
    /// the integration test on the real spawn path without changing runtime
    /// command selection.
    pub fn new_with_fixture_program(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        program: PathBuf,
    ) -> Self {
        Self::new_with_fixture_program_and_admission(
            journal,
            verbose,
            debug,
            outbound,
            program,
            Admission::new(Arc::new(SystemMemoryProbe)),
        )
    }

    /// Construct a batch dispatcher whose native children receive scoped
    /// process environment supplied by the batch owner.
    pub(crate) fn new_batch_with_environment(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        describe_workers: usize,
        child_environment: solstone_core_system::process::ChildLaunchContext,
        marker_policy: BatchMarkerPolicy,
    ) -> Self {
        Self::new_batch_inner(
            journal,
            verbose,
            debug,
            outbound,
            BatchContext {
                describe_workers: Some(describe_workers),
                child_environment,
                marker_policy,
            },
            resolve_program_from_current_exe(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_batch_with_executor(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        describe_workers: usize,
        child_environment: solstone_core_system::process::ChildLaunchContext,
        executor: Arc<dyn SenseExecutor>,
        marker_policy: BatchMarkerPolicy,
    ) -> Self {
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            resolve_program_from_current_exe(),
            BatchContext {
                describe_workers: Some(describe_workers),
                child_environment,
                marker_policy,
            },
            None,
            Some(executor),
        )
    }

    /// Construct a batch dispatcher that uses the built fixture handler.
    #[cfg(feature = "test-stubs")]
    pub(crate) fn new_batch_with_fixture_program(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        describe_workers: usize,
        program: PathBuf,
        marker_policy: BatchMarkerPolicy,
    ) -> Self {
        Self::new_batch_inner(
            journal,
            verbose,
            debug,
            outbound,
            BatchContext {
                describe_workers: Some(describe_workers),
                child_environment: solstone_core_system::process::ChildLaunchContext::default(),
                marker_policy,
            },
            Ok(program),
        )
    }

    fn new_batch_inner(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        batch: BatchContext,
        program: Result<PathBuf, DispatcherResolveError>,
    ) -> Self {
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            program,
            batch,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Distinct construction seams remain independently injectable.
    fn new_inner(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        admission: Admission,
        program: Result<PathBuf, DispatcherResolveError>,
        batch: BatchContext,
        hosted_parent: Option<Arc<HostedServiceParentRuntime>>,
        executor: Option<Arc<dyn SenseExecutor>>,
    ) -> Self {
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));
        let tally = Arc::new(JobTally::new());
        let cleanups = Arc::new(Mutex::new(Vec::new()));
        let mut queues = HashMap::new();
        let mut worker_handles = Vec::new();
        for handler in HANDLERS {
            let workers = if handler == "describe" {
                batch
                    .describe_workers
                    .unwrap_or_else(|| resolve_concurrency(&read_config(&journal), handler))
            } else {
                resolve_concurrency(&read_config(&journal), handler)
            };
            let queue = Arc::new(Mutex::new(HandlerQueue::new(workers)));
            queues.insert(handler, Arc::clone(&queue));
            for _ in 0..workers {
                let queue = Arc::clone(&queue);
                let state = Arc::clone(&state);
                let outbound = outbound.clone();
                let context = WorkerContext {
                    journal: journal.clone(),
                    admission: admission.clone(),
                    verbose,
                    debug,
                    batch: batch.clone(),
                    program: program.clone(),
                    tally: Arc::clone(&tally),
                    cleanups: Arc::clone(&cleanups),
                    hosted_parent: hosted_parent.clone(),
                    executor: executor.clone(),
                };
                let worker = thread::Builder::new()
                    .name(format!("{handler}-worker"))
                    .spawn(move || worker(queue, state, outbound, context))
                    .expect("sense worker thread");
                worker_handles.push(worker);
            }
        }
        Self {
            archive_admission: Mutex::new(()),
            journal,
            state,
            outbound,
            queues,
            workers: Mutex::new(worker_handles),
            cleanups,
            admission,
            batch,
            tally,
            executor,
        }
    }

    pub(crate) fn journal(&self) -> &Path {
        &self.journal
    }

    /// Queued, running and lease-deferred archive files share one admission
    /// bound. Live files remain free to enter the higher-priority queue.
    pub fn archive_pending_files(&self) -> usize {
        archive_pending_files_in(&self.state.lock().expect("sense state"))
    }

    pub fn handle(&self, message: &solstone_core_callosum::CallosumEnvelope) {
        if message
            .extra
            .get("meta")
            .and_then(|m| m.get("audio_archive"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            let _guard = self.archive_admission.lock().expect("archive admission");
            let capacity =
                crate::archive::MAX_PENDING_FILES.saturating_sub(self.archive_pending_files());
            if capacity == 0 {
                return;
            }
            let mut bounded = message.clone();
            if let Some(files) = bounded.extra.get_mut("files").and_then(Value::as_array_mut) {
                files.truncate(capacity);
            }
            self.handle_inner(&bounded);
        } else {
            self.handle_inner(message);
        }
    }

    fn handle_inner(&self, message: &solstone_core_callosum::CallosumEnvelope) {
        if message.tract != "observe" || message.event != "observing" {
            return;
        }
        let Some(day) = message
            .extra
            .get("day")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        else {
            return;
        };
        let Some(segment) = message
            .extra
            .get("segment")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        else {
            return;
        };
        let Some(files) = message.extra.get("files").and_then(Value::as_array) else {
            return;
        };
        let stream = message
            .extra
            .get("stream")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let cid = message
            .extra
            .get("cid")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let source = message
            .extra
            .get("source")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let batch = message
            .extra
            .get("batch")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut meta = message
            .extra
            .get("meta")
            .and_then(Value::as_object)
            .cloned();
        if let Some(stream) = &stream {
            meta.get_or_insert_default()
                .insert("stream".into(), json!(stream));
        }
        let context = SegmentContext {
            key: SegmentKey {
                day: day.to_owned(),
                stream: stream.clone(),
                segment: segment.to_owned(),
            },
            cid,
            source,
            batch,
            meta,
        };
        let dir = segment_dir(&self.journal, day, stream.as_deref(), segment);
        let paths: Vec<_> = files
            .iter()
            .filter_map(Value::as_str)
            .map(|file| dir.join(file))
            .collect();
        {
            let mut state = self.state.lock().expect("sense state");
            if state.stopping
                || state.segments.contains_key(&context.key)
                || state
                    .lease_deferred
                    .iter()
                    .any(|pending| pending.context.key == context.key)
            {
                return;
            }
            // A batch walk already holds this day's lease for its whole run
            // (`batch.rs` takes it before scanning). Taking it again here
            // through a second descriptor contends with our own process and
            // would silently drop every batch segment, so only the live
            // service path admits per day; a repair batch and live sensing
            // still exclude each other through the batch-level lease.
            if (!batch || is_archive(&context)) && !state.day_leases.contains_key(&context.key.day)
            {
                match crate::lease::acquire_sense_day_lease(&self.journal, &context.key.day) {
                    Ok(Some(lease)) => {
                        state.day_leases.insert(context.key.day.clone(), lease);
                    }
                    Ok(None) | Err(_) => {
                        // Contention with repair or unreadable: a same-day
                        // repair batch (or another live instance) holds the
                        // lease right now. Defer and retry on the next status
                        // tick instead of dropping this capture — see
                        // `retry_lease_deferred`.
                        state.lease_deferred.push(LeaseDeferredAdmission {
                            context: context.clone(),
                            paths,
                            first_deferred_at: Instant::now(),
                        });
                        return;
                    }
                }
            }
            state
                .segments
                .insert(context.key.clone(), SegmentState::new(context.clone()));
        }
        self.admit_after_lease(context, paths);
    }

    /// Dispatch an admitted segment's files to their handlers. Called
    /// immediately from [`Self::handle`] once the day lease is held, and
    /// again from [`Self::retry_lease_deferred`] once a deferred admission
    /// finally acquires it.
    fn admit_after_lease(&self, context: SegmentContext, paths: Vec<PathBuf>) {
        let config = read_config(&self.journal);
        let deferred = !context.batch && processing_deferred(&config);
        // With no thinking engine chosen, a live segment still gets the work
        // that needs none (transcription); describe and depict ask the engine
        // and wait, and the daily sense repair picks them up once an engine is
        // chosen, because it re-enters every file that has no output yet.
        let engine_hold = !context.batch && no_thinking_engine(&config);
        let describe_workers = self
            .batch
            .describe_workers
            .unwrap_or_else(|| resolve_concurrency(&config, "describe"));
        let registry = default_registry(crate::config::describe_per_proc_jobs(
            &config,
            describe_workers,
            &self.journal,
        ));
        if deferred {
            self.emit_observed(
                &context.key,
                Some(if engine_hold { "no_engine" } else { "deferred" }),
            );
            return;
        }
        let mut admitted = Vec::new();
        let mut held = false;
        for path in paths {
            let Some(spec) = match_handler(&self.journal, &path, &registry) else {
                continue;
            };
            if engine_hold && !runs_without_thinking_engine(spec.name) {
                held = true;
                continue;
            }
            let mut state = self.state.lock().expect("sense state");
            if !reserve_pending_file(&mut state.pending_files, spec.name, &path) {
                continue;
            }
            state
                .segments
                .get_mut(&context.key)
                .expect("admitted segment")
                .pending
                .insert(path.clone());
            admitted.push((path, spec));
        }
        if held
            && let Some(segment) = self
                .state
                .lock()
                .expect("sense state")
                .segments
                .get_mut(&context.key)
        {
            segment.hold_note = Some("no_engine");
        }
        for (path, spec) in admitted {
            let handler = spec.name;
            let inherited = match inherited_priority_text(&self.batch.child_environment.environment)
            {
                Ok(inherited) => inherited,
                Err(error) => {
                    self.tally.failure();
                    self.state
                        .lock()
                        .expect("sense state")
                        .health
                        .failure(&error);
                    complete(
                        &self.state,
                        &self.outbound,
                        &self.journal,
                        &context.key,
                        Some(&path),
                        Some(error),
                        None,
                        self.batch.marker_policy,
                    );
                    continue;
                }
            };
            let priority = match classify_sense_priority(
                &context.key.day,
                &context.key.segment,
                context.meta.as_ref(),
                inherited.as_deref(),
            ) {
                Ok(priority) => priority,
                Err(error) => {
                    self.tally.failure();
                    self.state
                        .lock()
                        .expect("sense state")
                        .health
                        .failure(&error);
                    complete(
                        &self.state,
                        &self.outbound,
                        &self.journal,
                        &context.key,
                        Some(&path),
                        Some(error),
                        None,
                        self.batch.marker_policy,
                    );
                    continue;
                }
            };
            let item = WorkItem {
                context: context.clone(),
                file_path: path.clone(),
                handler: handler.to_owned(),
                queued_at: SystemTime::now(),
            };
            let queued = if let Some(queue) = self.queues.get(handler) {
                let mut queue = queue.lock().expect("sense handler queue");
                queue.enqueue(Job {
                    item,
                    spec,
                    config: config.clone(),
                    priority,
                })
            } else {
                false
            };
            if !queued {
                self.tally.failure();
                let reason = format!("{handler} pool unavailable");
                self.state
                    .lock()
                    .expect("sense state")
                    .health
                    .failure(&reason);
                complete(
                    &self.state,
                    &self.outbound,
                    &self.journal,
                    &context.key,
                    Some(&path),
                    Some(reason),
                    None,
                    self.batch.marker_policy,
                );
            }
        }
        if self
            .state
            .lock()
            .expect("sense state")
            .segments
            .get(&context.key)
            .is_some_and(|v| v.pending.is_empty())
        {
            self.emit_observed(
                &context.key,
                Some(if engine_hold {
                    "no_engine"
                } else {
                    "no handlers"
                }),
            );
        }
    }

    fn emit_observed(&self, key: &SegmentKey, note: Option<&str>) {
        if !complete(
            &self.state,
            &self.outbound,
            &self.journal,
            key,
            None,
            None,
            note,
            self.batch.marker_policy,
        ) {
            self.tally.failure();
        }
    }

    /// Retry every live segment still waiting on a contended sense-day lease.
    ///
    /// Runs on every status tick (every 5s; see `service.rs`). A repair
    /// batch normally releases the lease within seconds to a couple of
    /// minutes of finishing its scan, so this typically resolves on the next
    /// tick or two; entries older than `LIVE_DAY_LEASE_RETRY_BUDGET` are
    /// abandoned and reported instead of retried forever.
    fn retry_lease_deferred(&self) {
        let _guard = self.archive_admission.lock().expect("archive admission");
        let due = {
            let mut state = self.state.lock().expect("sense state");
            if state.lease_deferred.is_empty() {
                return;
            }
            std::mem::take(&mut state.lease_deferred)
        };
        let mut still_pending = Vec::new();
        for pending in due {
            if pending.first_deferred_at.elapsed() >= LIVE_DAY_LEASE_RETRY_BUDGET {
                self.abandon_lease_deferred(pending);
                continue;
            }
            let acquired = {
                let mut state = self.state.lock().expect("sense state");
                if state.day_leases.contains_key(&pending.context.key.day) {
                    true
                } else {
                    match crate::lease::acquire_sense_day_lease(
                        &self.journal,
                        &pending.context.key.day,
                    ) {
                        Ok(Some(lease)) => {
                            state
                                .day_leases
                                .insert(pending.context.key.day.clone(), lease);
                            true
                        }
                        Ok(None) | Err(_) => false,
                    }
                }
            };
            if !acquired {
                still_pending.push(pending);
                continue;
            }
            let already_admitted = {
                let mut state = self.state.lock().expect("sense state");
                if state.segments.contains_key(&pending.context.key) {
                    true
                } else {
                    state.segments.insert(
                        pending.context.key.clone(),
                        SegmentState::new(pending.context.clone()),
                    );
                    false
                }
            };
            if !already_admitted {
                self.admit_after_lease(pending.context, pending.paths);
            }
        }
        if !still_pending.is_empty() {
            self.state
                .lock()
                .expect("sense state")
                .lease_deferred
                .extend(still_pending);
        }
    }

    /// A deferred admission outlived `LIVE_DAY_LEASE_RETRY_BUDGET` without
    /// ever acquiring the day lease. Report it instead of leaving it silent:
    /// count it as a failure and emit an `observed` event carrying the
    /// `lease_timeout` note, the same channel every other completion (and
    /// the existing `deferred`/`no handlers` notes) already uses.
    fn abandon_lease_deferred(&self, pending: LeaseDeferredAdmission) {
        {
            let mut state = self.state.lock().expect("sense state");
            if !state.segments.contains_key(&pending.context.key) {
                state.segments.insert(
                    pending.context.key.clone(),
                    SegmentState::new(pending.context.clone()),
                );
            }
            state.health.failure("lease_timeout");
        }
        self.tally.failure();
        self.emit_observed(&pending.context.key, Some("lease_timeout"));
    }

    pub fn status(&self) {
        self.retry_lease_deferred();
        let state = self.state.lock().expect("sense state");
        let mut fields = state
            .health
            .beacon(state.pending_files.len(), self.admission.state());
        fields.insert(
            "archive_pending_queue_depth".into(),
            json!(archive_pending_files_in(&state)),
        );
        let _ = self.outbound.send(Outbound {
            tract: "observe",
            event: "status",
            fields,
        });
    }
    /// Stops workers after their active job is boundedly terminated. Queued
    /// work is intentionally not completed during shutdown, matching sense.py.
    pub fn stop(&self) {
        self.state.lock().expect("sense state").stopping = true;
    }
    /// Returns whether every worker and cleanup thread joined without panic.
    pub fn stop_and_wait(&self) -> bool {
        self.stop();
        let pending_by_segment = {
            let state = self.state.lock().expect("sense state");
            let mut grouped: HashMap<PathBuf, (String, String, Option<String>, Vec<String>)> =
                HashMap::new();
            for (handler, file_path) in &state.pending_files {
                let Some(dir) = file_path.parent() else {
                    continue;
                };
                let dir_buf = dir.to_path_buf();
                let key_opt = state.segments.keys().find(|k| {
                    segment_dir(&self.journal, &k.day, k.stream.as_deref(), &k.segment) == dir_buf
                });
                if let Some(key) = key_opt {
                    let entry = grouped.entry(dir_buf).or_insert_with(|| {
                        (
                            key.day.clone(),
                            key.segment.clone(),
                            key.stream.clone(),
                            Vec::new(),
                        )
                    });
                    if !entry.3.contains(handler) {
                        entry.3.push(handler.clone());
                    }
                }
            }
            grouped
        };
        for (dir, (day, segment, stream, mut handlers)) in pending_by_segment {
            handlers.sort();
            if dir.is_dir() {
                let mut extra = serde_json::Map::new();
                extra.insert("day".into(), json!(day));
                extra.insert("segment".into(), json!(segment));
                if let Some(stream) = stream {
                    extra.insert("stream".into(), json!(stream));
                }
                extra.insert("handlers".into(), json!(handlers));
                extra.insert("reason".into(), json!("shutdown"));
                let envelope = solstone_core_callosum::CallosumEnvelope {
                    tract: "observe".to_string(),
                    event: "interrupted".to_string(),
                    ts: Some(chrono::Utc::now().timestamp_millis()),
                    extra,
                };
                if self.execution_cancelled() {
                    continue;
                }
                if let Err(error) = solstone_core_callosum::append_durable_event_checked(
                    &dir,
                    &solstone_core_callosum::DurableEvent::Callosum(envelope),
                    &|| {
                        if self.execution_cancelled() {
                            Err("Sense cancelled".into())
                        } else {
                            Ok(())
                        }
                    },
                ) {
                    log::warn!(
                        "failed to record observe.interrupted event to {}: {error}",
                        dir.display()
                    );
                }
            }
        }
        self.join_workers()
    }

    pub(crate) fn execution_cancelled(&self) -> bool {
        self.executor
            .as_ref()
            .is_some_and(|executor| executor.cancelled())
    }

    pub(crate) fn is_idle(&self) -> bool {
        let state = self.state.lock().expect("sense state");
        state.pending_files.is_empty() && state.segments.is_empty()
    }
    fn join_workers(&self) -> bool {
        let mut joined = true;
        let workers = std::mem::take(&mut *self.workers.lock().expect("sense workers"));
        for worker in workers {
            joined &= worker.join().is_ok();
        }
        let cleanups = std::mem::take(&mut *self.cleanups.lock().expect("sense cleanups"));
        for cleanup in cleanups {
            joined &= cleanup.join().is_ok();
        }
        joined
    }
}

fn resolve_program_from_current_exe() -> Result<PathBuf, DispatcherResolveError> {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return Err(DispatcherResolveError::CurrentExe {
                message: error.to_string(),
            });
        }
    };
    resolve_dispatcher_in(executable.parent().unwrap_or_else(|| Path::new(".")))
}

struct HandlerQueue {
    jobs: Vec<Job>,
    worker_count: usize,
    closed: bool,
}

impl HandlerQueue {
    fn new(worker_count: usize) -> Self {
        Self {
            jobs: Vec::new(),
            worker_count,
            closed: false,
        }
    }

    fn enqueue(&mut self, job: Job) -> bool {
        if self.closed || self.worker_count == 0 {
            return false;
        }
        self.jobs.push(job);
        true
    }

    fn pop_best(&mut self) -> Option<Job> {
        let best_index = self
            .jobs
            .iter()
            .enumerate()
            .min_by_key(|(_, job)| job.priority.sort_key())
            .map(|(index, _)| index)?;
        Some(self.jobs.remove(best_index))
    }

    fn requeue_if_preempted(&mut self, job: &mut Option<Job>) -> bool {
        let Some(current) = job.as_ref() else {
            return false;
        };
        let strictly_better = self
            .jobs
            .iter()
            .any(|queued| queued.priority.sort_key() < current.priority.sort_key());
        if strictly_better {
            self.jobs.push(job.take().unwrap());
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn close(&mut self) {
        self.closed = true;
    }
}

pub(crate) fn reserve_pending_file(
    pending_files: &mut HashSet<(String, PathBuf)>,
    handler: &str,
    path: &Path,
) -> bool {
    pending_files.insert((handler.to_owned(), path.to_path_buf()))
}

fn inherited_priority_text(
    child_environment: &BTreeMap<OsString, OsString>,
) -> Result<Option<String>, String> {
    let key = OsStr::new(WORK_PRIORITY_ENV);
    if let Some(value) = child_environment.get(key) {
        value
            .to_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| format!("{WORK_PRIORITY_ENV} contains invalid UTF-8"))
    } else {
        match std::env::var(WORK_PRIORITY_ENV) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                Err(format!("{WORK_PRIORITY_ENV} contains invalid UTF-8"))
            }
        }
    }
}

fn classify_sense_priority(
    day: &str,
    segment: &str,
    meta: Option<&Map<String, Value>>,
    inherited: Option<&str>,
) -> Result<WorkPriority, String> {
    let date_part = day.parse::<u64>().unwrap_or(0);
    let time_part = segment
        .get(..6)
        .filter(|prefix| prefix.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|prefix| prefix.parse::<u64>().ok())
        .unwrap_or(0);
    let source_time = date_part
        .saturating_mul(1_000_000)
        .saturating_add(time_part);

    let has_import_id = meta
        .and_then(|m| m.get("import_id"))
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty());

    if has_import_id {
        return Ok(WorkPriority {
            band: WorkBand::Import,
            source_time,
        });
    }

    if let Some(text) = inherited {
        return text.parse::<WorkPriority>();
    }

    Ok(WorkPriority {
        band: WorkBand::Now,
        source_time,
    })
}

fn build_child_environment(
    segment: &str,
    meta: Option<&Map<String, Value>>,
    queued_at: SystemTime,
    child_launch_context: &ChildLaunchContext,
    priority: WorkPriority,
) -> BTreeMap<OsString, OsString> {
    let mut environment = BTreeMap::<OsString, OsString>::new();
    environment.insert("SOL_SEGMENT".into(), segment.to_owned().into());
    if let Some(meta) = meta {
        environment.insert(
            "SEGMENT_META".into(),
            serde_json::to_string(meta).unwrap_or_default().into(),
        );
    }
    environment.insert(
        "SOL_QUEUE_WAIT_MS".into(),
        events::queue_wait_ms(queued_at).to_string().into(),
    );
    environment.extend(child_launch_context.environment.clone());
    environment.insert(WORK_PRIORITY_ENV.into(), priority.to_string().into());
    environment
}

fn worker(
    queue: Arc<Mutex<HandlerQueue>>,
    state: Arc<Mutex<State>>,
    outbound: mpsc::Sender<Outbound>,
    context: WorkerContext,
) {
    loop {
        let stopping = {
            let state = state.lock().expect("sense state");
            state.stopping
        };
        if stopping {
            return;
        }
        let job = {
            let mut queue = queue.lock().expect("sense handler queue");
            queue.pop_best()
        };
        match job {
            Some(job) => run_job(job, &queue, &state, &outbound, &context),
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn run_job(
    job: Job,
    queue: &Arc<Mutex<HandlerQueue>>,
    state: &Arc<Mutex<State>>,
    outbound: &mpsc::Sender<Outbound>,
    worker_context: &WorkerContext,
) {
    let admission_stopping = || {
        let state = state.lock().expect("sense state");
        state.stopping
    };
    let stage = job.item.handler.clone();
    let segment_context = job.item.context.clone();
    let key = segment_context.key.clone();
    let file = job.item.file_path.clone();
    if let Some(executor) = &worker_context.executor {
        let admitted = worker_context.admission.wait(
            &stage,
            &job.config,
            || admission_stopping() || executor.cancelled(),
            |available, floor| {
                let _ = outbound.send(Outbound {
                    tract: "observe",
                    event: "memory_throttle_started",
                    fields: events::throttle_started(
                        &stage,
                        available / (1024 * 1024),
                        floor / (1024 * 1024),
                    ),
                });
            },
            |waited| {
                let _ = outbound.send(Outbound {
                    tract: "observe",
                    event: "memory_throttle_completed",
                    fields: events::throttle_completed(&stage, waited),
                });
            },
        );
        if !admitted {
            return;
        }
        let mut job_slot = Some(job);
        {
            let mut q = queue.lock().expect("sense handler queue");
            if q.requeue_if_preempted(&mut job_slot) {
                return;
            }
        }
        let job = job_slot.expect("job was not preempted");
        let cancel =
            std::sync::atomic::AtomicBool::new(state.lock().expect("sense state").stopping);
        let result = if executor.cancelled() {
            SenseJobResult::Hold
        } else {
            executor.execute(&job.item.handler, &file, &segment_context, None, &cancel)
        };
        let result = if executor.cancelled() {
            SenseJobResult::Hold
        } else if result == SenseJobResult::Finished(0)
            && unfinished_output(&file, &job.item.handler).is_some()
        {
            SenseJobResult::Finished(1)
        } else {
            result
        };
        match result {
            SenseJobResult::Finished(0) => {
                state.lock().expect("sense state").health.success();
                let marker_succeeded = complete_checked(
                    state,
                    outbound,
                    &worker_context.journal,
                    &key,
                    Some(&file),
                    None,
                    None,
                    worker_context.batch.marker_policy,
                    &|| executor.cancelled(),
                );
                if marker_succeeded {
                    worker_context.tally.success();
                } else {
                    worker_context.tally.failure();
                }
                return;
            }
            SenseJobResult::Finished(code) => {
                let stopped = state.lock().expect("sense state").stopping;
                if !stopped {
                    state
                        .lock()
                        .expect("sense state")
                        .health
                        .failure(&format!("{} exit {code}", job.item.handler));
                }
                worker_context.tally.failure();
                let outcome = format!("{} exit {code}", job.item.handler);
                complete_checked(
                    state,
                    outbound,
                    &worker_context.journal,
                    &key,
                    Some(&file),
                    Some(outcome),
                    None,
                    worker_context.batch.marker_policy,
                    &|| executor.cancelled(),
                );
                return;
            }
            SenseJobResult::Hold => {
                worker_context.tally.defer();
                let mut state = state.lock().expect("sense state");
                state
                    .pending_files
                    .retain(|(_, candidate)| candidate != &file);
                let empty = if let Some(segment) = state.segments.get_mut(&key) {
                    segment.hold_note = Some("handler_deferred");
                    segment.pending.remove(&file);
                    segment.pending.is_empty()
                } else {
                    false
                };
                if empty {
                    state.segments.remove(&key);
                    if !state
                        .segments
                        .keys()
                        .any(|candidate| candidate.day == key.day)
                    {
                        state.day_leases.remove(&key.day);
                    }
                }
                return;
            }
        }
    }
    let program = match &worker_context.program {
        Ok(program) => program,
        Err(error) => {
            worker_context.tally.failure();
            complete(
                state,
                outbound,
                &worker_context.journal,
                &key,
                Some(&file),
                Some(format!("{} {error}", job.item.handler)),
                None,
                worker_context.batch.marker_policy,
            );
            return;
        }
    };
    let admitted = worker_context.admission.wait(
        &stage,
        &job.config,
        admission_stopping,
        |available, floor| {
            let _ = outbound.send(Outbound {
                tract: "observe",
                event: "memory_throttle_started",
                fields: events::throttle_started(
                    &stage,
                    available / (1024 * 1024),
                    floor / (1024 * 1024),
                ),
            });
        },
        |waited| {
            let _ = outbound.send(Outbound {
                tract: "observe",
                event: "memory_throttle_completed",
                fields: events::throttle_completed(&stage, waited),
            });
        },
    );
    if !admitted {
        return;
    }
    let mut job_slot = Some(job);
    {
        let mut q = queue.lock().expect("sense handler queue");
        if q.requeue_if_preempted(&mut job_slot) {
            return;
        }
    }
    let job = job_slot.expect("job was not preempted");
    let command = command_for(
        &job.spec,
        program,
        &file,
        worker_context.verbose,
        worker_context.debug,
    );
    let reference = chrono::Utc::now().timestamp_millis().to_string();
    let _ = outbound.send(Outbound {
        tract: "observe",
        event: "detected",
        fields: events::detected(
            &worker_context.journal,
            &file,
            &job.item.handler,
            &reference,
            &segment_context,
        ),
    });
    let environment = build_child_environment(
        &segment_context.key.segment,
        segment_context.meta.as_ref(),
        job.item.queued_at,
        &worker_context.batch.child_environment,
        job.priority,
    );
    let hosted_launch_id = format!(
        "sense-{}-{reference}-{}",
        job.item.handler,
        HOSTED_CHILD_LAUNCH_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    );
    let options = SpawnOptions {
        journal_root: worker_context.journal.clone(),
        reference,
        day: Some(segment_context.key.day.clone()),
        sink: None,
        environment,
    };
    let launch = ManagedLaunchRequest {
        #[cfg(windows)]
        read_file_grants: worker_context
            .batch
            .child_environment
            .read_file_grants
            .clone(),
        command,
        options,
    };
    let launched = match worker_context.hosted_parent.as_ref() {
        Some(parent) => launch_managed_hosted(
            Disposition::InheritedParentScope,
            launch,
            parent.child_launch_provenance(hosted_launch_id),
        ),
        None => {
            #[cfg(unix)]
            {
                solstone_core_system::lifecycle::hosted_child_launch_provenance(
                    hosted_launch_id,
                    Duration::from_secs(5),
                )
                .map_err(|error| {
                    solstone_core_system::process::LaunchError::Admission(error.to_string())
                })
                .and_then(|provenance| match provenance {
                    Some(provenance) => {
                        launch_managed_hosted(Disposition::InheritedParentScope, launch, provenance)
                    }
                    None => launch_managed_request(Disposition::InheritedParentScope, launch),
                })
            }
            #[cfg(not(unix))]
            launch_managed_request(Disposition::InheritedParentScope, launch)
        }
    };
    let Ok(authority) = launched else {
        worker_context.tally.failure();
        complete(
            state,
            outbound,
            &worker_context.journal,
            &key,
            Some(&file),
            Some(format!("{} spawn failed", job.item.handler)),
            None,
            worker_context.batch.marker_policy,
        );
        return;
    };
    let Ok(mut process) = authority.into_managed() else {
        worker_context.tally.failure();
        complete(
            state,
            outbound,
            &worker_context.journal,
            &key,
            Some(&file),
            Some(format!(
                "{} launch authority was not managed",
                job.item.handler
            )),
            None,
            worker_context.batch.marker_policy,
        );
        return;
    };
    let cap = resolve_max_runtime(&job.config, &job.item.handler);
    let deadline = std::time::Instant::now() + cap;
    let exit = loop {
        match process.poll() {
            Ok(Some(code)) => break Some(code),
            Ok(None) => {}
            Err(_) => break None,
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        if state.lock().expect("sense state").stopping {
            if let Err(error) = process.terminate(Duration::from_secs(2)) {
                let _ = termination_detail(&job.item.handler, error);
            }
            break process.poll().ok().flatten().or(Some(-15));
        }
        thread::sleep(Duration::from_millis(50));
    };
    if exit == Some(0)
        && let Some(reason) = unfinished_output(&file, &job.item.handler)
    {
        worker_context.tally.failure();
        state.lock().expect("sense state").health.failure(&reason);
        complete(
            state,
            outbound,
            &worker_context.journal,
            &key,
            Some(&file),
            Some(reason),
            None,
            worker_context.batch.marker_policy,
        );
        cleanup_async(process, &worker_context.cleanups, true);
        return;
    }
    if exit != Some(0) {
        eprintln!(
            "sense: handler={} file={} outcome={} log={}",
            job.item.handler,
            file.display(),
            match exit {
                Some(PROVIDER_BLOCKED) => "deferred (exit 69)".to_owned(),
                Some(code) => format!("failed (exit {code})"),
                None => "watchdog or process wait failed".to_owned(),
            },
            process.log_path().display(),
        );
    }
    let outcome = match exit {
        Some(PROVIDER_BLOCKED) => {
            if let Some(segment) = state.lock().expect("sense state").segments.get_mut(&key) {
                segment.hold_note = Some("handler_deferred");
            }
            let marker_succeeded = complete(
                state,
                outbound,
                &worker_context.journal,
                &key,
                Some(&file),
                None,
                Some("handler_deferred"),
                worker_context.batch.marker_policy,
            );
            if marker_succeeded {
                worker_context.tally.defer();
            } else {
                worker_context.tally.failure();
            }
            cleanup_async(process, &worker_context.cleanups, true);
            return;
        }
        Some(0) => {
            state.lock().expect("sense state").health.success();
            let marker_succeeded = complete(
                state,
                outbound,
                &worker_context.journal,
                &key,
                Some(&file),
                None,
                None,
                worker_context.batch.marker_policy,
            );
            if marker_succeeded {
                worker_context.tally.success();
            } else {
                worker_context.tally.failure();
            }
            cleanup_async(process, &worker_context.cleanups, false);
            return;
        }
        Some(code) => {
            let stopped = state.lock().expect("sense state").stopping;
            if !stopped {
                notify_failure(
                    outbound,
                    &job.item.handler,
                    &file,
                    &process.log_path(),
                    "Error",
                );
                state
                    .lock()
                    .expect("sense state")
                    .health
                    .failure(&format!("{} exit {code}", job.item.handler));
            }
            worker_context.tally.failure();
            format!("{} exit {code}", job.item.handler)
        }
        None => {
            let termination = process.terminate(Duration::from_secs(2));
            let mut reason = format!(
                "{} watchdog_timeout after {}s",
                job.item.handler,
                cap.as_secs()
            );
            if let Err(error) = termination
                && let Some(detail) = termination_detail(&job.item.handler, error)
            {
                reason = format!("{reason}; {detail}");
            }
            notify_failure(
                outbound,
                &job.item.handler,
                &file,
                &process.log_path(),
                "Timeout",
            );
            state.lock().expect("sense state").health.failure(&reason);
            worker_context.tally.failure();
            reason
        }
    };
    complete(
        state,
        outbound,
        &worker_context.journal,
        &key,
        Some(&file),
        Some(outcome),
        None,
        worker_context.batch.marker_policy,
    );
    cleanup_async(process, &worker_context.cleanups, true);
}

/// Exit zero cannot finish a file that the ordinary scanner would admit again.
pub fn unfinished_output(file: &Path, handler: &str) -> Option<String> {
    use solstone_core_processing_record::{
        read_processing_record_header, should_reenter_analysis_output,
    };
    let output = file.with_extension("jsonl");
    match std::fs::metadata(&output) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Some(format!(
                "{handler} exited 0 without a regular output: {}",
                output.display()
            ));
        }
        Err(error) => {
            return Some(format!(
                "{handler} exited 0 without readable output {}: {error}",
                output.display()
            ));
        }
    }
    let record = read_processing_record_header(&output);
    if record
        .as_ref()
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        == Some("failed")
        || should_reenter_analysis_output(record.as_ref(), &output, handler)
    {
        let reason = record
            .as_ref()
            .and_then(|value| value.get("reason_code"))
            .and_then(Value::as_str)
            .unwrap_or("analysis output incomplete");
        return Some(format!(
            "{handler} exited 0 with unfinished output ({reason}): {}",
            output.display()
        ));
    }
    None
}

fn cleanup_async(
    process: ManagedProcess,
    cleanups: &Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    diagnostic: bool,
) {
    let handoff = Arc::new(Mutex::new(Some(process)));
    let cleanup_handoff = Arc::clone(&handoff);
    let spawned = thread::Builder::new()
        .name("sense-process-cleanup".into())
        .spawn(move || {
            if let Some(mut process) = cleanup_handoff.lock().expect("cleanup handoff").take() {
                process.cleanup();
                if diagnostic {
                    report_diagnostic_tail(&process.log_path());
                }
            }
        });
    if let Ok(cleanup) = spawned {
        let finished = {
            let mut cleanups = cleanups.lock().expect("sense cleanups");
            let mut active = Vec::with_capacity(cleanups.len() + 1);
            let mut finished = Vec::new();
            for cleanup in std::mem::take(&mut *cleanups) {
                if cleanup.is_finished() {
                    finished.push(cleanup);
                } else {
                    active.push(cleanup);
                }
            }
            active.push(cleanup);
            *cleanups = active;
            finished
        };
        for cleanup in finished {
            let _ = cleanup.join();
        }
    } else {
        // terminate-then-cleanup is the Drop-equivalent path when the cleanup
        // thread cannot be spawned; terminate() reaps descendants before join.
        let mut process = handoff
            .lock()
            .expect("cleanup handoff")
            .take()
            .expect("cleanup process");
        let _ = process.terminate(SERVICE_SHUTDOWN_TIMEOUT);
        process.cleanup();
        if diagnostic {
            report_diagnostic_tail(&process.log_path());
        }
    }
}

fn report_diagnostic_tail(path: &Path) {
    use std::io::{Read, Seek, SeekFrom};
    let tail = (|| -> std::io::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(4096)))?;
        let mut bytes = Vec::new();
        file.take(4096).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })();
    match tail {
        Ok(tail) => eprintln!(
            "sense: handler diagnostic log={} tail={tail:?}",
            path.display()
        ),
        Err(error) => eprintln!(
            "sense: handler diagnostic log={} unreadable: {error}",
            path.display()
        ),
    }
}

fn termination_detail(handler: &str, error: TerminationError) -> Option<String> {
    match error {
        TerminationError::ProcessTreeNotReaped { reason, survivors } => {
            eprintln!(
                "sense: process-tree-not-reaped handler={handler} reason={reason} survivors={}",
                survivors.len()
            );
            Some(format!(
                "process_tree_not_reaped reason={reason} survivors={}",
                survivors.len()
            ))
        }
        error => {
            eprintln!("sense: managed termination failed handler={handler}: {error}");
            None
        }
    }
}

fn notify_failure(
    outbound: &mpsc::Sender<Outbound>,
    handler: &str,
    file: &Path,
    log: &Path,
    suffix: &str,
) {
    let fields = Map::from_iter([
        (
            String::from("message"),
            json!(format!(
                "{} {} for {}",
                capitalize(handler),
                suffix.to_lowercase(),
                file.file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or_default()
            )),
        ),
        (
            String::from("title"),
            json!(format!("{} {suffix}", capitalize(handler))),
        ),
        (String::from("icon"), json!(icon(handler))),
        (String::from("app"), json!("sense")),
        (
            String::from("action"),
            json!(format!("/app/health?log={}", log.display())),
        ),
    ]);
    let _ = outbound.send(Outbound {
        tract: "notification",
        event: "show",
        fields,
    });
}
fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}
fn icon(handler: &str) -> &'static str {
    match handler {
        "transcribe" => "mic-vocal",
        "describe" => "eye",
        _ => "bot",
    }
}

/// Whether a handler's work needs no thinking engine, so a live segment runs
/// it even while none is chosen. Transcription picks its own speech-to-text
/// backend; describe and depict ask the chosen engine.
fn runs_without_thinking_engine(handler: &str) -> bool {
    handler == "transcribe"
}

#[allow(clippy::too_many_arguments)] // One explicit call carries the full segment-completion boundary.
fn complete(
    state: &Arc<Mutex<State>>,
    outbound: &mpsc::Sender<Outbound>,
    journal: &Path,
    key: &SegmentKey,
    file: Option<&Path>,
    error: Option<String>,
    note: Option<&str>,
    marker_policy: BatchMarkerPolicy,
) -> bool {
    complete_checked(
        state,
        outbound,
        journal,
        key,
        file,
        error,
        note,
        marker_policy,
        &|| false,
    )
}

#[allow(clippy::too_many_arguments)]
fn complete_checked(
    state: &Arc<Mutex<State>>,
    outbound: &mpsc::Sender<Outbound>,
    journal: &Path,
    key: &SegmentKey,
    file: Option<&Path>,
    error: Option<String>,
    note: Option<&str>,
    marker_policy: BatchMarkerPolicy,
    cancelled: &dyn Fn() -> bool,
) -> bool {
    let completed = {
        let mut state = state.lock().expect("sense state");
        if cancelled() {
            return false;
        }
        if let Some(file) = file {
            state
                .pending_files
                .retain(|(_, candidate)| candidate != file);
        }
        let empty = {
            let Some(segment) = state.segments.get_mut(key) else {
                return true;
            };
            if let Some(error) = error {
                eprintln!(
                    "sense: day={} stream={} segment={} file={} error={error}",
                    key.day,
                    key.stream.as_deref().unwrap_or("default"),
                    key.segment,
                    file.map(|path| path.display().to_string())
                        .unwrap_or_default()
                );
                segment.errors.push(error);
            }
            if let Some(file) = file {
                segment.pending.remove(file);
            }
            segment.pending.is_empty()
        };
        let removed = if empty {
            state.segments.remove(key)
        } else {
            None
        };
        if !state
            .segments
            .keys()
            .any(|candidate| candidate.day == key.day)
        {
            state.day_leases.remove(&key.day);
        }
        removed
    };
    let mut marker_succeeded = true;
    if let Some(mut segment) = completed {
        if cancelled() {
            return false;
        }
        if marker_policy == BatchMarkerPolicy::AdvanceStream
            && segment.hold_note != Some("handler_deferred")
            && let Err(error) = solstone_core_journal_io::health_marker::bump_stream_marker_checked(
                journal,
                &segment.context.key.day,
                &|| {
                    if cancelled() {
                        Err("Sense cancelled".into())
                    } else {
                        Ok(())
                    }
                },
            )
        {
            marker_succeeded = false;
            segment
                .errors
                .push(format!("stream marker update failed: {error}"));
        }
        if cancelled() {
            return false;
        }
        let _ = outbound.send(Outbound {
            tract: "observe",
            event: "observed",
            fields: events::observed(&segment, note.or(segment.hold_note)),
        });
    }
    marker_succeeded
}

#[cfg(test)]
mod tests {

    #[test]
    fn deferred_jobs_are_counted_separately_from_failures() {
        let tally = super::JobTally::new();
        tally.success();
        tally.defer();
        tally.failure();
        assert_eq!(tally.snapshot(), (1, 3));
        assert_eq!(tally.deferred(), 1);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn zero_exit_cannot_hide_retryable_or_exhausted_failed_output() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join("audio.flac");
        let output = raw.with_extension("jsonl");
        for attempts in [1, 3] {
            std::fs::write(&output, format!("{}\n", serde_json::json!({"_solstone_processing": {
                "state":"failed", "handler":"transcribe", "reason_code":"decode_transient", "attempts":attempts
            }}))).unwrap();
            assert!(
                super::unfinished_output(&raw, "transcribe")
                    .unwrap()
                    .contains("decode_transient")
            );
        }
        std::fs::write(&output, "{}\n").unwrap();
        assert!(super::unfinished_output(&raw, "describe").is_some());
        std::fs::write(&output, "{}\n{\"timestamp\":0}\n").unwrap();
        assert!(super::unfinished_output(&raw, "describe").is_none());
    }
    #[cfg(feature = "full-tests")]
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    use serde_json::json;
    use solstone_core_callosum::CallosumEnvelope;
    use solstone_core_journal_io::{
        HealthMarkerKind, HealthMarkerState, bump_stream_marker, read_health_marker,
    };
    use solstone_core_system::process::Descendant;

    use super::*;

    fn observing(stream: &str, segment: &str) -> CallosumEnvelope {
        CallosumEnvelope {
            tract: "observe".into(),
            event: "observing".into(),
            ts: None,
            extra: Map::from_iter([
                ("day".into(), json!("20260812")),
                ("stream".into(), json!(stream)),
                ("segment".into(), json!(segment)),
                ("files".into(), json!(["ignored.txt"])),
            ]),
        }
    }

    #[test]
    fn batch_segments_admit_while_the_batch_owner_holds_the_day_lease() {
        let temp = tempfile::tempdir().expect("temp journal");
        let held = crate::lease::acquire_sense_day_lease(temp.path(), "20260812")
            .expect("lease acquire")
            .expect("lease is free");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            BatchContext::default(),
            None,
            None,
        );
        let mut live = observing("stream1", "120000_1");
        live.extra.insert("files".into(), json!([]));
        let mut batch = observing("stream1", "130000_1");
        batch.extra.insert("files".into(), json!([]));
        batch.extra.insert("batch".into(), json!(true));

        // The live path contends with the held lease and admits nothing.
        dispatcher.handle(&live);
        assert!(
            receiver.try_recv().is_err(),
            "live segment must not admit under a held day lease"
        );
        assert!(
            !dispatcher
                .state
                .lock()
                .unwrap()
                .day_leases
                .contains_key("20260812")
        );

        // The batch path is the lease owner and completes its segment.
        dispatcher.handle(&batch);
        let event = receiver
            .try_recv()
            .expect("batch segment completes with an observed event");
        assert_eq!(event.event, "observed");
        assert_eq!(event.fields.get("segment"), Some(&json!("130000_1")));
        assert!(
            !dispatcher
                .state
                .lock()
                .unwrap()
                .day_leases
                .contains_key("20260812")
        );
        drop(held);
        assert!(dispatcher.stop_and_wait());
    }

    #[test]
    fn deferred_live_admission_retries_and_completes_once_the_lease_frees_up() {
        let temp = tempfile::tempdir().expect("temp journal");
        let held = crate::lease::acquire_sense_day_lease(temp.path(), "20260812")
            .expect("lease acquire")
            .expect("lease is free");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            BatchContext::default(),
            None,
            None,
        );
        let mut live = observing("stream1", "120000_1");
        live.extra.insert("files".into(), json!([]));

        // A same-day repair batch (or another live instance) already holds
        // the lease: this reproduces the ja1r fresh-use gate failure, where
        // the startup sense repair held today's lease while the gate's one
        // capture landed.
        dispatcher.handle(&live);
        assert!(
            receiver.try_recv().is_err(),
            "a contended live segment must not admit synchronously"
        );
        assert_eq!(
            dispatcher.state.lock().unwrap().lease_deferred.len(),
            1,
            "a contended live segment is deferred, not dropped"
        );

        // The repair batch finishes and releases the lease.
        drop(held);

        // The next status tick (every 5s in production) retries it.
        dispatcher.status();
        let event = receiver
            .try_recv()
            .expect("the deferred segment completes once the lease frees up");
        assert_eq!(event.event, "observed");
        assert_eq!(event.fields.get("segment"), Some(&json!("120000_1")));
        assert!(
            dispatcher.state.lock().unwrap().lease_deferred.is_empty(),
            "a resolved retry must not stay queued"
        );
        assert!(dispatcher.stop_and_wait());
    }

    #[test]
    fn deferred_live_admission_abandoned_after_the_retry_budget_reports_lease_timeout() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            BatchContext::default(),
            None,
            None,
        );
        let context = SegmentContext {
            key: SegmentKey {
                day: "20260812".into(),
                stream: Some("stream1".into()),
                segment: "120000_1".into(),
            },
            cid: None,
            source: None,
            batch: false,
            meta: None,
        };
        dispatcher
            .state
            .lock()
            .unwrap()
            .lease_deferred
            .push(LeaseDeferredAdmission {
                context,
                paths: Vec::new(),
                first_deferred_at: Instant::now()
                    - (LIVE_DAY_LEASE_RETRY_BUDGET + Duration::from_secs(1)),
            });

        dispatcher.status();

        let event = receiver
            .try_recv()
            .expect("an abandoned deferral is reported, not silently dropped");
        assert_eq!(event.event, "observed");
        assert_eq!(event.fields.get("segment"), Some(&json!("120000_1")));
        assert_eq!(
            dispatcher.tally.snapshot(),
            (1, 1),
            "an abandoned deferral counts as a failure"
        );
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status follows deferred completion");
        assert_eq!(status.fields["recent_error_count"], 1);
        assert_eq!(status.fields["last_error_reason"], "lease_timeout");
        assert_eq!(status.fields["last_successful_sync"], Value::Null);
        dispatcher.status();
        let next_status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("next status");
        assert_eq!(next_status.fields["recent_error_count"], 1);
        assert_eq!(next_status.fields["last_error_reason"], "lease_timeout");
        assert!(dispatcher.state.lock().unwrap().lease_deferred.is_empty());
        assert!(dispatcher.stop_and_wait());
    }

    #[test]
    fn stop_and_wait_reports_a_panicked_worker() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, _receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher
            .workers
            .lock()
            .expect("sense workers")
            .push(thread::spawn(|| panic!("worker panic")));

        assert!(!dispatcher.stop_and_wait());
    }

    #[test]
    fn same_segment_name_in_two_streams_emits_two_observed_events() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}}}"#,
        )
        .expect("settings");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher.handle(&observing("one", "120000_1"));
        dispatcher.handle(&observing("two", "120000_1"));
        let events = receiver.try_iter().collect::<Vec<_>>();
        let observed = events
            .iter()
            .filter(|event| event.event == "observed")
            .collect::<Vec<_>>();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].fields["stream"], "one");
        assert_eq!(observed[1].fields["stream"], "two");
    }

    #[test]
    fn no_engine_and_deferred_notes_do_not_count_as_dispatch_failures() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher.handle(&observing("one", "120000_1"));
        let event = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(event.fields["note"], "no_engine");
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 0);

        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}},"processing":{"mode":"deferred"}}"#,
        )
        .expect("settings");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher.handle(&observing("one", "120001_1"));
        let event = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(event.fields["note"], "deferred");
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 0);
    }

    #[test]
    fn deferred_batch_bypasses_live_hold_while_live_still_emits_observed() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}},"processing":{"mode":"deferred"}}"#,
        )
        .unwrap();
        for batch in [false, true] {
            let (outbound, receiver) = mpsc::channel();
            let dispatcher =
                SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
            let mut message = observing("one", "120000_1");
            message.extra.insert("batch".into(), json!(batch));
            message.extra.insert("files".into(), json!(["ignored.txt"]));
            dispatcher.handle(&message);
            let event = receiver
                .try_iter()
                .find(|event| event.event == "observed")
                .unwrap();
            assert_eq!(
                event.fields["note"],
                if batch { "no handlers" } else { "deferred" }
            );
            assert!(dispatcher.stop_and_wait());
        }
    }

    #[test]
    fn no_handlers_note_does_not_count_as_dispatch_failure() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}}}"#,
        )
        .expect("settings");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        let mut message = observing("one", "120002_1");
        message.extra.insert("files".into(), json!(["ignored.txt"]));
        dispatcher.handle(&message);
        let event = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(event.fields["note"], "no handlers");
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 0);
    }

    #[test]
    fn unavailable_worker_pool_completes_segment_with_error() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}}}"#,
        )
        .expect("settings");
        let file = temp
            .path()
            .join("chronicle/20260812/one/120000_1/audio.flac");
        std::fs::create_dir_all(file.parent().expect("segment")).expect("segment");
        std::fs::write(&file, b"audio").expect("audio");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher
            .queues
            .get("transcribe")
            .expect("transcribe queue")
            .lock()
            .expect("queue")
            .close();
        let mut message = observing("one", "120000_1");
        message.extra.insert("files".into(), json!(["audio.flac"]));
        dispatcher.handle(&message);
        let events = receiver.try_iter().collect::<Vec<_>>();
        let observed = events
            .iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(observed.fields["error"], true);
        assert!(
            observed.fields["errors"][0]
                .as_str()
                .expect("error")
                .contains("transcribe pool unavailable")
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event == "observed")
                .count(),
            1,
            "one pool admission failure is reported once"
        );
        let state = dispatcher.state.lock().expect("state");
        assert!(state.segments.is_empty());
        assert!(state.pending_files.is_empty());
        drop(state);
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 1);
        assert_eq!(
            status.fields["last_error_reason"],
            "transcribe pool unavailable"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn handler_exit_failure_remains_counted_once_in_the_beacon() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}}}"#,
        )
        .expect("settings");
        let audio = temp
            .path()
            .join("chronicle/20260812/one/120000_1/audio.flac");
        std::fs::create_dir_all(audio.parent().expect("segment")).expect("segment");
        std::fs::write(&audio, b"audio").expect("audio");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_with_fixture_program(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            // /usr/bin/false exists on macOS and Linux; /bin/false is Linux-only.
            PathBuf::from("/usr/bin/false"),
        );
        let mut message = observing("one", "120000_1");
        message.extra.insert("files".into(), json!(["audio.flac"]));
        dispatcher.handle(&message);
        let observed = loop {
            let event = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("handler error completion");
            if event.event == "observed" {
                break event;
            }
        };
        assert_eq!(observed.fields["error"], true);
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 1);
        assert_eq!(status.fields["last_error_reason"], "transcribe exit 1");
        assert!(dispatcher.stop_and_wait());
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn unavailable_pool_does_not_remove_a_multifile_segment_before_other_work_starts() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}}}"#,
        )
        .expect("settings");
        let segment = temp.path().join("chronicle/20260812/one/120000_1");
        std::fs::create_dir_all(&segment).expect("segment");
        std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
        std::fs::write(segment.join("screen.webm"), b"screen").expect("screen");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_with_fixture_program(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            PathBuf::from("/bin/true"),
        );
        dispatcher
            .queues
            .get("transcribe")
            .expect("transcribe queue")
            .lock()
            .expect("queue")
            .close();
        let mut message = observing("one", "120000_1");
        message
            .extra
            .insert("files".into(), json!(["audio.flac", "screen.webm"]));
        dispatcher.handle(&message);
        let mut describe_detected = false;
        let observed = loop {
            let event = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("segment completes");
            if event.event == "detected" {
                assert_eq!(event.fields["handler"], "describe");
                describe_detected = true;
            }
            if event.event == "observed" {
                break event;
            }
        };
        assert!(describe_detected, "describe fixture path emits detected");
        assert_eq!(observed.fields["error"], true);
        assert!(
            observed.fields["errors"][0]
                .as_str()
                .expect("error")
                .contains("transcribe pool unavailable")
        );
        assert!(
            receiver
                .recv_timeout(Duration::from_millis(200))
                .map_or(true, |event| event.event != "observed"),
            "segment emits observed exactly once"
        );
    }

    /// A fresh journal with no thinking engine chosen (an Apple Silicon Mac
    /// under the local bar, or a computer without a usable GPU) still
    /// transcribes a new live segment. Screen work, which asks the engine,
    /// waits for the daily sense repair once an engine is chosen.
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn no_engine_live_segment_is_transcribed_and_screen_work_waits() {
        let temp = tempfile::tempdir().expect("temp journal");
        let segment = temp.path().join("chronicle/20260812/one/120000_1");
        std::fs::create_dir_all(&segment).expect("segment");
        std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
        std::fs::write(segment.join("screen.webm"), b"screen").expect("screen");
        assert!(crate::config::no_thinking_engine(&read_config(temp.path())));
        let fixture = temp.path().join("transcribe-fixture");
        std::fs::write(&fixture, "#!/bin/sh\nprintf '{}\\n' > \"${2%.*}.jsonl\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_with_fixture_program(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            fixture,
        );
        let mut message = observing("one", "120000_1");
        message
            .extra
            .insert("files".into(), json!(["audio.flac", "screen.webm"]));
        dispatcher.handle(&message);
        let mut detected = Vec::new();
        let observed = loop {
            let event = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("segment completes");
            if event.event == "detected" {
                detected.push(event.fields["handler"].clone());
            }
            if event.event == "observed" {
                break event;
            }
        };
        assert_eq!(detected, vec![json!("transcribe")]);
        assert!(!observed.fields.contains_key("error"), "{observed:?}");
        assert_eq!(observed.fields["note"], "no_engine");
        let waiting = crate::batch::scan_unprocessed(
            temp.path(),
            &temp.path().join("chronicle/20260812"),
            Some("120000_1"),
            None,
            None,
        )
        .expect("scan");
        assert!(
            waiting
                .iter()
                .any(|work| work.handler == "describe" && work.path.ends_with("screen.webm")),
            "screen work stays for the sense repair: {waiting:?}"
        );
        assert!(dispatcher.stop_and_wait());
    }

    /// The owner's deferred-processing choice still holds every handler,
    /// transcription included, whether or not an engine is chosen.
    #[test]
    fn deferred_processing_still_holds_transcription_without_an_engine() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"processing":{"mode":"deferred"}}"#,
        )
        .expect("settings");
        let segment = temp.path().join("chronicle/20260812/one/120000_1");
        std::fs::create_dir_all(&segment).expect("segment");
        std::fs::write(segment.join("audio.flac"), b"audio").expect("audio");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        let mut message = observing("one", "120000_1");
        message.extra.insert("files".into(), json!(["audio.flac"]));
        dispatcher.handle(&message);
        let events = receiver.try_iter().collect::<Vec<_>>();
        assert!(events.iter().all(|event| event.event != "detected"));
        let observed = events
            .iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(observed.fields["note"], "no_engine");
        assert!(dispatcher.state.lock().unwrap().pending_files.is_empty());
        assert!(dispatcher.stop_and_wait());
    }

    #[test]
    fn only_transcription_runs_without_a_thinking_engine() {
        assert!(runs_without_thinking_engine("transcribe"));
        assert!(!runs_without_thinking_engine("describe"));
        assert!(!runs_without_thinking_engine("depict"));
    }

    #[test]
    fn process_tree_not_reaped_becomes_a_path_free_segment_error_detail() {
        let detail = termination_detail(
            "describe",
            TerminationError::ProcessTreeNotReaped {
                reason: "survived_sigkill",
                survivors: vec![Descendant {
                    pid: 99,
                    ppid: 1,
                    pgid: Some(99),
                    uid: 501,
                }],
            },
        )
        .expect("tree detail");
        assert_eq!(
            detail,
            "process_tree_not_reaped reason=survived_sigkill survivors=1"
        );
        assert!(!detail.contains('/'));
    }

    #[test]
    fn tree_not_reaped_detail_reaches_observed_error_channel() {
        let temp = tempfile::tempdir().expect("journal");
        let (outbound, receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));
        let key = SegmentKey {
            day: "20260812".into(),
            stream: None,
            segment: "120000_1".into(),
        };
        state.lock().expect("state").segments.insert(
            key.clone(),
            SegmentState::new(SegmentContext {
                key: key.clone(),
                cid: None,
                source: None,
                batch: false,
                meta: None,
            }),
        );
        complete(
            &state,
            &outbound,
            temp.path(),
            &key,
            None,
            Some("describe watchdog_timeout; process_tree_not_reaped reason=survived_sigkill survivors=1".into()),
            None,
            BatchMarkerPolicy::AdvanceStream,
        );
        let observed = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(observed.fields["error"], true);
        assert!(
            observed.fields["errors"][0]
                .as_str()
                .expect("error")
                .contains("process_tree_not_reaped")
        );
    }

    #[test]
    fn idle_status_tick_preserves_dispatch_errors_and_does_not_claim_a_sync() {
        let temp = tempfile::tempdir().expect("journal");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher
            .state
            .lock()
            .expect("state")
            .health
            .failure("describe exit 7");
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 1);
        assert_eq!(status.fields["last_error_reason"], "describe exit 7");
        assert_eq!(status.fields["last_successful_sync"], Value::Null);
    }

    #[test]
    fn idle_status_tick_preserves_an_existing_success_timestamp() {
        let temp = tempfile::tempdir().expect("journal");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher
            .state
            .lock()
            .expect("state")
            .health
            .last_successful_sync = Some(123);
        dispatcher.status();
        let status = receiver
            .try_iter()
            .find(|event| event.event == "status")
            .expect("status");
        assert_eq!(status.fields["recent_error_count"], 0);
        assert_eq!(status.fields["last_successful_sync"], 123);
    }

    #[test]
    fn marker_policy_is_independent_of_batch_presentation() {
        let temp = tempfile::tempdir().expect("journal");
        let (outbound, _receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));
        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("default".into()),
            segment: "live".into(),
        };
        state.lock().expect("state").segments.insert(
            key.clone(),
            SegmentState::new(SegmentContext {
                key: key.clone(),
                cid: None,
                source: None,
                batch: false,
                meta: None,
            }),
        );
        complete(
            &state,
            &outbound,
            temp.path(),
            &key,
            None,
            None,
            Some("no handlers"),
            BatchMarkerPolicy::AdvanceStream,
        );
        let marker = temp.path().join("chronicle/20260812/health/stream.updated");
        assert!(marker.is_file());
        std::fs::remove_file(&marker).expect("remove live marker");
        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("default".into()),
            segment: "batch".into(),
        };
        state.lock().expect("state").segments.insert(
            key.clone(),
            SegmentState::new(SegmentContext {
                key: key.clone(),
                cid: None,
                source: None,
                batch: true,
                meta: None,
            }),
        );
        complete(
            &state,
            &outbound,
            temp.path(),
            &key,
            None,
            None,
            Some("no handlers"),
            BatchMarkerPolicy::AdvanceStream,
        );
        assert!(marker.is_file());
        std::fs::remove_file(&marker).expect("remove batch marker");
        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("default".into()),
            segment: "whole-day".into(),
        };
        state.lock().expect("state").segments.insert(
            key.clone(),
            SegmentState::new(SegmentContext {
                key: key.clone(),
                cid: None,
                source: None,
                batch: true,
                meta: None,
            }),
        );
        complete(
            &state,
            &outbound,
            temp.path(),
            &key,
            None,
            None,
            Some("no handlers"),
            BatchMarkerPolicy::EnclosedWholeDay,
        );
        assert!(!marker.exists());
    }

    #[test]
    fn enclosed_whole_day_batch_does_not_advance_its_admitted_generation() {
        let temp = tempfile::tempdir().expect("journal");
        assert_eq!(
            bump_stream_marker(temp.path(), "20260812").expect("admitted marker"),
            1
        );
        let (outbound, _receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));
        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("default".into()),
            segment: "whole-day".into(),
        };
        state.lock().expect("state").segments.insert(
            key.clone(),
            SegmentState::new(SegmentContext {
                key: key.clone(),
                cid: None,
                source: None,
                batch: true,
                meta: None,
            }),
        );

        complete(
            &state,
            &outbound,
            temp.path(),
            &key,
            None,
            None,
            Some("whole-day sense"),
            BatchMarkerPolicy::EnclosedWholeDay,
        );

        assert!(matches!(
            read_health_marker(temp.path(), "20260812", HealthMarkerKind::Stream)
                .expect("stream marker"),
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 1
        ));
    }

    #[test]
    fn live_completion_reports_stream_marker_failure() {
        let temp = tempfile::tempdir().expect("journal");
        let health = temp.path().join("chronicle/20260812/health");
        std::fs::create_dir_all(health.parent().expect("chronicle day")).expect("chronicle day");
        std::fs::write(&health, b"not a directory").expect("blocked health path");
        let (outbound, receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));
        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("default".into()),
            segment: "live".into(),
        };
        state.lock().expect("state").segments.insert(
            key.clone(),
            SegmentState::new(SegmentContext {
                key: key.clone(),
                cid: None,
                source: None,
                batch: false,
                meta: None,
            }),
        );

        let marker_succeeded = complete(
            &state,
            &outbound,
            temp.path(),
            &key,
            None,
            None,
            None,
            BatchMarkerPolicy::AdvanceStream,
        );
        assert!(!marker_succeeded);

        let observed = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(observed.fields["error"], true);
        assert!(
            observed.fields["errors"][0]
                .as_str()
                .expect("marker failure")
                .contains("stream marker update failed")
        );
    }

    #[test]
    fn unresolved_dispatcher_completes_a_matched_file_without_detected() {
        let temp = tempfile::tempdir().expect("temp journal");
        std::fs::create_dir_all(temp.path().join("config")).expect("config");
        std::fs::write(
            temp.path().join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"openai"}}}"#,
        )
        .expect("settings");
        let file = temp
            .path()
            .join("chronicle/20260812/one/120000_1/audio.flac");
        std::fs::create_dir_all(file.parent().expect("segment")).expect("segment");
        std::fs::write(&file, b"audio").expect("audio");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/opt/solstone-core-journal"),
            }),
            BatchContext::default(),
            None,
            None,
        );
        let mut message = observing("one", "120000_1");
        message.extra.insert("files".into(), json!(["audio.flac"]));
        dispatcher.handle(&message);
        let mut saw_detected = false;
        let observed = loop {
            let event = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("segment completes");
            if event.event == "detected" {
                saw_detected = true;
            }
            if event.event == "observed" {
                break event;
            }
        };
        assert!(!saw_detected, "resolve failure must not emit detected");
        assert_eq!(observed.fields["error"], true);
        let error = observed.fields["errors"][0].as_str().expect("error");
        assert!(error.starts_with("transcribe "));
        assert!(error.contains("/opt/solstone-core-journal"));
        let (failed, ran) = dispatcher.tally.snapshot();
        assert!(failed >= 1);
        assert!(ran >= 1);
    }

    #[test]
    fn stop_and_wait_records_observe_interrupted_event_to_uncompleted_segment() {
        let temp = tempfile::tempdir().expect("temp journal");
        let seg_dir = temp.path().join("chronicle/20260812/stream1/120000_1");
        std::fs::create_dir_all(&seg_dir).expect("segment dir");
        let (outbound, _receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            BatchContext::default(),
            None,
            None,
        );

        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("stream1".into()),
            segment: "120000_1".into(),
        };
        let context = SegmentContext {
            key: key.clone(),
            cid: None,
            source: None,
            batch: false,
            meta: None,
        };
        let mut seg_state = SegmentState::new(context);
        let audio_path = seg_dir.join("test.wav");
        seg_state.pending.insert(audio_path.clone());
        {
            let mut state = dispatcher.state.lock().unwrap();
            state
                .pending_files
                .insert(("transcribe".into(), audio_path));
            state.segments.insert(key, seg_state);
        }

        dispatcher.stop_and_wait();

        let events_path = seg_dir.join("events.jsonl");
        assert!(events_path.exists());
        let contents = std::fs::read_to_string(events_path).expect("read events.jsonl");
        assert!(contents.contains("\"tract\":\"observe\""));
        assert!(contents.contains("\"event\":\"interrupted\""));
        assert!(contents.contains("\"reason\":\"shutdown\""));
        assert!(contents.contains("\"handlers\":[\"transcribe\"]"));
    }

    #[test]
    fn sense_dispatcher_interruption_ignores_completed_and_unobserved_segments() {
        let temp = tempfile::tempdir().expect("temp journal");
        let completed_dir = temp.path().join("chronicle/20260812/stream1/120000_1");
        let unobserved_dir = temp.path().join("chronicle/20260812/stream1/130000_1");
        std::fs::create_dir_all(&completed_dir).expect("completed dir");
        std::fs::create_dir_all(&unobserved_dir).expect("unobserved dir");

        let (outbound, _receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            BatchContext::default(),
            None,
            None,
        );

        let key = SegmentKey {
            day: "20260812".into(),
            stream: Some("stream1".into()),
            segment: "120000_1".into(),
        };
        let context = SegmentContext {
            key: key.clone(),
            cid: None,
            source: None,
            batch: false,
            meta: None,
        };
        let mut seg_state = SegmentState::new(context);
        let audio_path = completed_dir.join("test.wav");
        seg_state.pending.insert(audio_path.clone());
        {
            let mut state = dispatcher.state.lock().unwrap();
            state
                .pending_files
                .insert(("transcribe".into(), audio_path.clone()));
            state.segments.insert(key.clone(), seg_state);
        }

        complete(
            &dispatcher.state,
            &dispatcher.outbound,
            temp.path(),
            &key,
            Some(&audio_path),
            None,
            None,
            BatchMarkerPolicy::AdvanceStream,
        );

        dispatcher.stop_and_wait();

        let completed_events = completed_dir.join("events.jsonl");
        if completed_events.exists() {
            let contents = std::fs::read_to_string(completed_events).unwrap();
            assert!(!contents.contains("\"event\":\"interrupted\""));
        }
        let unobserved_events = unobserved_dir.join("events.jsonl");
        assert!(!unobserved_events.exists());
    }

    #[test]
    fn sense_dispatcher_interruption_cost_under_shutdown_timeout() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, _receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new_inner(
            temp.path().to_path_buf(),
            false,
            false,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            BatchContext::default(),
            None,
            None,
        );

        let segment_count = 256;
        {
            let mut state = dispatcher.state.lock().unwrap();
            for i in 0..segment_count {
                let seg_name = format!("12{:04}_1", i);
                let seg_dir = temp.path().join("chronicle/20260812").join(&seg_name);
                std::fs::create_dir_all(&seg_dir).expect("create seg dir");
                let key = SegmentKey {
                    day: "20260812".into(),
                    stream: None,
                    segment: seg_name,
                };
                let context = SegmentContext {
                    key: key.clone(),
                    cid: None,
                    source: None,
                    batch: false,
                    meta: None,
                };
                let mut seg_state = SegmentState::new(context);
                let audio_path = seg_dir.join("audio.wav");
                let video_path = seg_dir.join("video.mp4");
                seg_state.pending.insert(audio_path.clone());
                seg_state.pending.insert(video_path.clone());
                state
                    .pending_files
                    .insert(("transcribe".into(), audio_path));
                state.pending_files.insert(("describe".into(), video_path));
                state.segments.insert(key, seg_state);
            }
        }

        let start = std::time::Instant::now();
        dispatcher.stop_and_wait();
        let elapsed = start.elapsed();
        assert!(
            elapsed < SERVICE_SHUTDOWN_TIMEOUT,
            "interruption cost ({:?}) must be well under shutdown timeout ({:?})",
            elapsed,
            SERVICE_SHUTDOWN_TIMEOUT
        );
    }

    fn dummy_spec(name: &'static str) -> HandlerSpec {
        HandlerSpec {
            name,
            patterns: &[],
            command: vec![name.to_owned()],
        }
    }

    fn test_job(handler: &'static str, file: &Path, priority: WorkPriority) -> Job {
        Job {
            item: WorkItem {
                context: SegmentContext {
                    key: SegmentKey {
                        day: "20261016".into(),
                        stream: None,
                        segment: "120000_1".into(),
                    },
                    cid: None,
                    source: None,
                    batch: false,
                    meta: None,
                },
                file_path: file.to_path_buf(),
                handler: handler.to_owned(),
                queued_at: SystemTime::now(),
            },
            spec: dummy_spec(handler),
            config: Map::new(),
            priority,
        }
    }

    #[test]
    fn live_now_is_popped_before_import_when_inserted_last() {
        let mut queue = HandlerQueue::new(1);
        let import_older = test_job(
            "transcribe",
            Path::new("imp1.flac"),
            "4:20261010080000".parse().unwrap(),
        );
        let import_newer = test_job(
            "transcribe",
            Path::new("imp2.flac"),
            "4:20261016120000".parse().unwrap(),
        );
        let live_old = test_job(
            "transcribe",
            Path::new("live.flac"),
            "1:20261001010000".parse().unwrap(),
        );

        assert!(queue.enqueue(import_older));
        assert!(queue.enqueue(import_newer));
        assert!(queue.enqueue(live_old));

        let popped = queue.pop_best().expect("job popped");
        assert_eq!(popped.priority.band, WorkBand::Now);
        assert_eq!(popped.priority.source_time, 20261001010000);
    }

    #[test]
    fn imports_popped_newest_source_time_first() {
        let mut queue = HandlerQueue::new(1);
        let import_day1 = test_job(
            "transcribe",
            Path::new("imp1.flac"),
            "4:20261010080000".parse().unwrap(),
        );
        let import_day2 = test_job(
            "transcribe",
            Path::new("imp2.flac"),
            "4:20261016120000".parse().unwrap(),
        );

        assert!(queue.enqueue(import_day1));
        assert!(queue.enqueue(import_day2));

        let first = queue.pop_best().expect("first job");
        assert_eq!(first.priority.to_string(), "4:20261016120000");

        let second = queue.pop_best().expect("second job");
        assert_eq!(second.priority.to_string(), "4:20261010080000");
    }

    #[test]
    fn classify_sense_priority_respects_rules() {
        let meta = Map::from_iter([("import_id".into(), json!("imp123"))]);
        let p1 =
            classify_sense_priority("20261016", "153045_12", Some(&meta), Some("1:1")).unwrap();
        assert_eq!(p1.band, WorkBand::Import);
        assert_eq!(p1.source_time, 20261016153045);

        let p2 = classify_sense_priority("20261016", "153045_12", None, Some("5:20261003120000"))
            .unwrap();
        assert_eq!(p2.band, WorkBand::Completing);
        assert_eq!(p2.source_time, 20261003120000);

        let err = classify_sense_priority("20261016", "153045_12", None, Some("not-a-priority"))
            .unwrap_err();
        assert_eq!(err, "not-a-priority".parse::<WorkPriority>().unwrap_err());

        let p3 = classify_sense_priority("20261016", "153045_12", None, None).unwrap();
        assert_eq!(p3.band, WorkBand::Now);
        assert_eq!(p3.source_time, 20261016153045);

        let p_utf8 = classify_sense_priority("20261016", "12345é", None, None).unwrap();
        assert_eq!(p_utf8.band, WorkBand::Now);
        assert_eq!(p_utf8.source_time, 20261016000000);

        let meta_utf8 = Map::from_iter([("import_id".into(), json!("imp"))]);
        let p_utf8_imp =
            classify_sense_priority("20261016", "12345é", Some(&meta_utf8), None).unwrap();
        assert_eq!(p_utf8_imp.band, WorkBand::Import);
        assert_eq!(p_utf8_imp.source_time, 20261016000000);
    }

    #[test]
    fn child_environment_sets_priority_and_preserves_keys() {
        let mut child_launch_context = ChildLaunchContext::default();
        child_launch_context
            .environment
            .insert("LD_LIBRARY_PATH".into(), "/opt/lib".into());

        let repair_priority = "5:20261003120000".parse::<WorkPriority>().unwrap();
        let env_repair = build_child_environment(
            "120000_1",
            None,
            SystemTime::now(),
            &child_launch_context,
            repair_priority,
        );
        assert_eq!(
            env_repair.get(OsStr::new(WORK_PRIORITY_ENV)).unwrap(),
            "5:20261003120000"
        );
        assert_eq!(
            env_repair.get(OsStr::new("LD_LIBRARY_PATH")).unwrap(),
            "/opt/lib"
        );

        let import_priority = "4:20261016153045".parse::<WorkPriority>().unwrap();
        let env_import = build_child_environment(
            "153045_12",
            None,
            SystemTime::now(),
            &child_launch_context,
            import_priority,
        );
        assert_eq!(
            env_import.get(OsStr::new(WORK_PRIORITY_ENV)).unwrap(),
            "4:20261016153045"
        );
        assert_eq!(
            env_import.get(OsStr::new("LD_LIBRARY_PATH")).unwrap(),
            "/opt/lib"
        );
    }

    #[test]
    fn reserve_enqueue_dedup_and_pending_length() {
        let mut pending_files = HashSet::new();
        let path = Path::new("audio.flac");
        let handler = "transcribe";

        assert!(reserve_pending_file(&mut pending_files, handler, path));
        assert_eq!(pending_files.len(), 1);

        assert!(!reserve_pending_file(&mut pending_files, handler, path));
        assert_eq!(pending_files.len(), 1);

        let mut queue = HandlerQueue::new(1);
        let job = test_job(handler, path, "1:20261016120000".parse().unwrap());
        assert!(queue.enqueue(job));
        assert_eq!(queue.jobs.len(), 1);

        let popped = queue.pop_best();
        assert!(popped.is_some());
        assert_eq!(queue.jobs.len(), 0);
        assert_eq!(pending_files.len(), 1);

        let mut job_slot = popped;
        let better_job = test_job(
            handler,
            Path::new("better.flac"),
            "1:20261016130000".parse().unwrap(),
        );
        assert!(queue.enqueue(better_job));
        assert!(queue.requeue_if_preempted(&mut job_slot));
        assert!(job_slot.is_none());
        assert_eq!(pending_files.len(), 1);
        assert_eq!(queue.jobs.len(), 2);
    }

    #[cfg(feature = "full-tests")]
    struct MockExecutor {
        called: Arc<AtomicBool>,
        result: SenseJobResult,
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn cancelled_completion_waiting_for_state_cannot_publish_a_marker_or_event() {
        for cancel in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let job = test_job(
                "depict",
                &temp.path().join("frame.png"),
                "1:20261016120000".parse().unwrap(),
            );
            let context = job.item.context;
            let key = context.key.clone();
            let mut segment = SegmentState::new(context);
            segment.pending.insert(job.item.file_path.clone());
            let state = Arc::new(Mutex::new(State {
                segments: HashMap::from([(key.clone(), segment)]),
                pending_files: HashSet::new(),
                health: Health::default(),
                stopping: false,
                day_leases: HashMap::new(),
                lease_deferred: Vec::new(),
            }));
            let held = state.lock().unwrap();
            let worker_state = Arc::clone(&state);
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&cancelled);
            let journal = temp.path().to_path_buf();
            let file = job.item.file_path;
            let worker_key = key.clone();
            let (outbound, events) = mpsc::channel();
            let (started, ready) = mpsc::channel();
            let (ended, done) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                started.send(()).unwrap();
                ended
                    .send(complete_checked(
                        &worker_state,
                        &outbound,
                        &journal,
                        &worker_key,
                        Some(&file),
                        None,
                        None,
                        BatchMarkerPolicy::AdvanceStream,
                        &|| flag.load(Ordering::Acquire),
                    ))
                    .unwrap();
            });
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(done.recv_timeout(Duration::from_millis(50)).is_err());
            cancelled.store(cancel, Ordering::Release);
            drop(held);
            assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), !cancel);
            worker.join().unwrap();
            assert_eq!(
                temp.path()
                    .join("chronicle")
                    .join(&key.day)
                    .join("health/stream.updated")
                    .exists(),
                !cancel
            );
            assert_eq!(events.try_recv().is_ok(), !cancel);
        }
    }

    #[cfg(feature = "full-tests")]
    impl SenseExecutor for MockExecutor {
        fn execute(
            &self,
            _handler: &str,
            path: &Path,
            _context: &SegmentContext,
            _attempt: Option<&solstone_core_system::work_retry::Attempt>,
            _cancel: &AtomicBool,
        ) -> SenseJobResult {
            self.called.store(true, Ordering::SeqCst);
            if self.result == SenseJobResult::Finished(0) {
                std::fs::write(
                    path.with_extension("jsonl"),
                    "{}\n{\"text\":\"callable fixture output\"}\n",
                )
                .unwrap();
            }
            self.result
        }
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn executor_hold_for_describe_defers_and_leaves_no_stream_marker() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, _receiver) = mpsc::channel();
        let tally = Arc::new(JobTally::new());
        let called = Arc::new(AtomicBool::new(false));
        let executor = Arc::new(MockExecutor {
            called: Arc::clone(&called),
            result: SenseJobResult::Hold,
        });
        let worker_context = WorkerContext {
            journal: temp.path().to_path_buf(),
            admission: Admission::new(Arc::new(SystemMemoryProbe)),
            verbose: false,
            debug: false,
            batch: BatchContext::default(),
            program: Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            tally: Arc::clone(&tally),
            cleanups: Arc::new(Mutex::new(Vec::new())),
            hosted_parent: None,
            executor: Some(executor),
        };
        let file = temp.path().join("frame.jpg");
        let job = test_job("describe", &file, "1:20261016120000".parse().unwrap());
        let key = job.item.context.key.clone();
        let queue = Arc::new(Mutex::new(HandlerQueue::new(1)));
        let mut segment_state = SegmentState::new(job.item.context.clone());
        segment_state.pending.insert(file.clone());
        let mut pending_files = HashSet::new();
        pending_files.insert(("describe".to_string(), file.clone()));
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::from([(key.clone(), segment_state)]),
            pending_files,
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));

        run_job(job, &queue, &state, &outbound, &worker_context);

        assert!(called.load(Ordering::SeqCst), "executor must be called");
        assert_eq!(tally.deferred(), 1);
        let (failed, _ran) = tally.snapshot();
        assert_eq!(failed, 0);
        let marker = temp
            .path()
            .join(format!("chronicle/{}/health/stream.updated", key.day));
        assert!(!marker.exists(), "temp journal must gain no stream marker");
        let s = state.lock().unwrap();
        assert!(
            !s.pending_files
                .contains(&("describe".to_string(), file.clone()))
        );
        assert!(!s.segments.contains_key(&key));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn executor_finished_zero_for_depict_completes_successfully() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, _receiver) = mpsc::channel();
        let tally = Arc::new(JobTally::new());
        let called = Arc::new(AtomicBool::new(false));
        let executor = Arc::new(MockExecutor {
            called: Arc::clone(&called),
            result: SenseJobResult::Finished(0),
        });
        let worker_context = WorkerContext {
            journal: temp.path().to_path_buf(),
            admission: Admission::new(Arc::new(SystemMemoryProbe)),
            verbose: false,
            debug: false,
            batch: BatchContext::default(),
            program: Err(DispatcherResolveError::Missing {
                path: PathBuf::from("/bin/nonexistent"),
            }),
            tally: Arc::clone(&tally),
            cleanups: Arc::new(Mutex::new(Vec::new())),
            hosted_parent: None,
            executor: Some(executor),
        };
        let file = temp.path().join("frame.jpg");
        let job = test_job("depict", &file, "1:20261016120000".parse().unwrap());
        let key = job.item.context.key.clone();
        let queue = Arc::new(Mutex::new(HandlerQueue::new(1)));
        let mut segment_state = SegmentState::new(job.item.context.clone());
        segment_state.pending.insert(file.clone());
        let mut pending_files = HashSet::new();
        pending_files.insert(("depict".to_string(), file.clone()));
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::from([(key.clone(), segment_state)]),
            pending_files,
            health: Health::default(),
            stopping: false,
            day_leases: HashMap::new(),
            lease_deferred: Vec::new(),
        }));

        run_job(job, &queue, &state, &outbound, &worker_context);

        assert!(called.load(Ordering::SeqCst), "executor must be called");
        let (failed, ran) = tally.snapshot();
        assert_eq!(failed, 0);
        assert_eq!(ran, 1);
        assert_eq!(tally.deferred(), 0);
    }
}
