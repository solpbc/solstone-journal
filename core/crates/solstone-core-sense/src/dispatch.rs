// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};
use solstone_core_system::process::{
    ManagedProcess, SERVICE_SHUTDOWN_TIMEOUT, SpawnOptions, TerminationError,
};

use crate::beacon::Health;
use crate::config::{
    HANDLERS, no_thinking_engine, processing_deferred, read_config, resolve_concurrency,
    resolve_max_runtime,
};
use crate::events;
use crate::memory::{Admission, SystemMemoryProbe};
use crate::registry::{
    DispatcherResolveError, HandlerSpec, command_for, default_registry, match_handler,
    resolve_dispatcher_in, segment_dir,
};
use crate::work::{SegmentContext, SegmentKey, SegmentState, WorkItem};

const PROVIDER_BLOCKED: i32 = 69;

#[derive(Debug, Clone)]
pub struct Outbound {
    pub tract: &'static str,
    pub event: &'static str,
    pub fields: Map<String, Value>,
}

struct Job {
    item: WorkItem,
    spec: HandlerSpec,
    config: Map<String, Value>,
}
struct State {
    segments: HashMap<SegmentKey, SegmentState>,
    pending_files: HashSet<(String, PathBuf)>,
    health: Health,
    stopping: bool,
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
    child_environment: BTreeMap<OsString, OsString>,
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
}

/// Per-run handler outcome counts. Incremented as `run_job` finishes, then
/// read after the dispatcher is idle. Not a rolling beacon.
pub(crate) struct JobTally {
    ran: AtomicUsize,
    failed: AtomicUsize,
}

impl JobTally {
    fn new() -> Self {
        Self {
            ran: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
        }
    }

    fn success(&self) {
        self.ran.fetch_add(1, Ordering::SeqCst);
    }

    fn failure(&self) {
        self.ran.fetch_add(1, Ordering::SeqCst);
        self.failed.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn snapshot(&self) -> (usize, usize) {
        (
            self.failed.load(Ordering::SeqCst),
            self.ran.load(Ordering::SeqCst),
        )
    }
}

pub struct SenseDispatcher {
    journal: PathBuf,
    state: Arc<Mutex<State>>,
    outbound: mpsc::Sender<Outbound>,
    pools: HashMap<&'static str, Mutex<WorkerPool>>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    cleanups: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    admission: Admission,
    batch: BatchContext,
    pub(crate) tally: Arc<JobTally>,
}

impl SenseDispatcher {
    pub fn new(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
    ) -> Self {
        Self::new_with_admission(
            journal,
            verbose,
            debug,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
        )
    }

    pub fn new_with_admission(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        admission: Admission,
    ) -> Self {
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            admission,
            resolve_program_from_current_exe(),
            BatchContext::default(),
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
        Self::new_inner(
            journal,
            verbose,
            debug,
            outbound,
            Admission::new(Arc::new(SystemMemoryProbe)),
            Ok(program),
            BatchContext::default(),
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
        child_environment: BTreeMap<OsString, OsString>,
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
                child_environment: BTreeMap::new(),
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
        )
    }

    fn new_inner(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        admission: Admission,
        program: Result<PathBuf, DispatcherResolveError>,
        batch: BatchContext,
    ) -> Self {
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
        }));
        let tally = Arc::new(JobTally::new());
        let cleanups = Arc::new(Mutex::new(Vec::new()));
        let mut pools = HashMap::new();
        let mut worker_handles = Vec::new();
        for handler in HANDLERS {
            let workers = if handler == "describe" {
                batch
                    .describe_workers
                    .unwrap_or_else(|| resolve_concurrency(&read_config(&journal), handler))
            } else {
                resolve_concurrency(&read_config(&journal), handler)
            };
            let mut senders = Vec::with_capacity(workers);
            for _ in 0..workers {
                let (sender, receiver) = mpsc::channel();
                senders.push(sender);
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
                };
                let worker = thread::Builder::new()
                    .name(format!("{handler}-worker"))
                    .spawn(move || worker(receiver, state, outbound, context))
                    .expect("sense worker thread");
                worker_handles.push(worker);
            }
            pools.insert(handler, Mutex::new(WorkerPool { senders, next: 0 }));
        }
        Self {
            journal,
            state,
            outbound,
            pools,
            workers: Mutex::new(worker_handles),
            cleanups,
            admission,
            batch,
            tally,
        }
    }

    pub fn handle(&self, message: &solstone_core_callosum::CallosumEnvelope) {
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
        let observer = message
            .extra
            .get("observer")
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
            observer,
            batch,
            meta,
        };
        let config = read_config(&self.journal);
        let deferred = !batch && (processing_deferred(&config) || no_thinking_engine(&config));
        let describe_workers = self
            .batch
            .describe_workers
            .unwrap_or_else(|| resolve_concurrency(&config, "describe"));
        let registry = default_registry(crate::config::describe_per_proc_jobs(
            &config,
            describe_workers,
            &self.journal,
        ));
        let dir = segment_dir(&self.journal, day, stream.as_deref(), segment);
        let paths: Vec<_> = files
            .iter()
            .filter_map(Value::as_str)
            .map(|file| dir.join(file))
            .collect();
        {
            let mut state = self.state.lock().expect("sense state");
            if state.segments.contains_key(&context.key) {
                return;
            }
            state
                .segments
                .insert(context.key.clone(), SegmentState::new(context.clone()));
        }
        if deferred {
            self.emit_observed(
                &context.key,
                Some(if no_thinking_engine(&config) {
                    "no_engine"
                } else {
                    "deferred"
                }),
            );
            return;
        }
        let mut admitted = Vec::new();
        for path in paths {
            let Some(spec) = match_handler(&self.journal, &path, &registry) else {
                continue;
            };
            let pending_key = (spec.name.to_owned(), path.clone());
            let mut state = self.state.lock().expect("sense state");
            if !state.pending_files.insert(pending_key) {
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
        for (path, spec) in admitted {
            let handler = spec.name;
            let item = WorkItem {
                context: context.clone(),
                file_path: path.clone(),
                handler: handler.to_owned(),
                queued_at: SystemTime::now(),
            };
            let queued = if let Some(pool) = self.pools.get(handler) {
                let mut pool = pool.lock().expect("sense worker pool");
                pool.send(Job {
                    item,
                    spec,
                    config: config.clone(),
                })
            } else {
                false
            };
            if !queued {
                complete(
                    &self.state,
                    &self.outbound,
                    &self.journal,
                    &context.key,
                    Some(&path),
                    Some(format!("{handler} pool unavailable")),
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
            self.emit_observed(&context.key, Some("no handlers"));
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
    pub fn status(&self) {
        let mut state = self.state.lock().expect("sense state");
        if state.pending_files.is_empty() {
            state.health.success();
        }
        let fields = state
            .health
            .beacon(state.pending_files.len(), self.admission.state());
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
    pub fn stop_and_wait(&self) {
        self.stop();
        self.join_workers();
    }

    pub(crate) fn is_idle(&self) -> bool {
        let state = self.state.lock().expect("sense state");
        state.pending_files.is_empty() && state.segments.is_empty()
    }
    fn join_workers(&self) {
        let workers = std::mem::take(&mut *self.workers.lock().expect("sense workers"));
        for worker in workers {
            let _ = worker.join();
        }
        let cleanups = std::mem::take(&mut *self.cleanups.lock().expect("sense cleanups"));
        for cleanup in cleanups {
            let _ = cleanup.join();
        }
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

struct WorkerPool {
    senders: Vec<mpsc::Sender<Job>>,
    next: usize,
}

impl WorkerPool {
    fn send(&mut self, job: Job) -> bool {
        let index = self.next % self.senders.len();
        self.next = self.next.wrapping_add(1);
        self.senders[index].send(job).is_ok()
    }
}

fn worker(
    receiver: mpsc::Receiver<Job>,
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
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(job) => run_job(job, &state, &outbound, &context),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn run_job(
    job: Job,
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
    let mut environment = BTreeMap::<OsString, OsString>::new();
    environment.insert(
        "SOL_SEGMENT".into(),
        segment_context.key.segment.clone().into(),
    );
    if let Some(observer) = &segment_context.observer {
        environment.insert("OBSERVER_NAME".into(), observer.into());
    }
    if let Some(meta) = &segment_context.meta {
        environment.insert(
            "SEGMENT_META".into(),
            serde_json::to_string(meta).unwrap_or_default().into(),
        );
    }
    environment.insert(
        "SOL_QUEUE_WAIT_MS".into(),
        events::queue_wait_ms(job.item.queued_at).to_string().into(),
    );
    environment.extend(worker_context.batch.child_environment.clone());
    let options = SpawnOptions {
        journal_root: worker_context.journal.clone(),
        reference,
        day: Some(segment_context.key.day.clone()),
        sink: None,
        environment,
    };
    let Ok(mut process) = ManagedProcess::spawn(command, options) else {
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
    let outcome = match exit {
        Some(PROVIDER_BLOCKED) => {
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
            cleanup_async(process, &worker_context.cleanups);
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
            cleanup_async(process, &worker_context.cleanups);
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
    cleanup_async(process, &worker_context.cleanups);
}

fn cleanup_async(process: ManagedProcess, cleanups: &Arc<Mutex<Vec<thread::JoinHandle<()>>>>) {
    let handoff = Arc::new(Mutex::new(Some(process)));
    let cleanup_handoff = Arc::clone(&handoff);
    let spawned = thread::Builder::new()
        .name("sense-process-cleanup".into())
        .spawn(move || {
            if let Some(mut process) = cleanup_handoff.lock().expect("cleanup handoff").take() {
                process.cleanup();
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
    let completed = {
        let mut state = state.lock().expect("sense state");
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
                segment.errors.push(error);
            }
            if let Some(file) = file {
                segment.pending.remove(file);
            }
            segment.pending.is_empty()
        };
        if empty {
            state.segments.remove(key)
        } else {
            None
        }
    };
    let mut marker_succeeded = true;
    if let Some(mut segment) = completed {
        if marker_policy == BatchMarkerPolicy::AdvanceStream
            && let Err(error) =
                solstone_core_journal_io::bump_stream_marker(journal, &segment.context.key.day)
        {
            marker_succeeded = false;
            segment
                .errors
                .push(format!("stream marker update failed: {error}"));
        }
        let _ = outbound.send(Outbound {
            tract: "observe",
            event: "observed",
            fields: events::observed(&segment, note),
        });
    }
    marker_succeeded
}

#[cfg(test)]
mod tests {
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
    fn deferred_and_no_engine_do_not_admit_a_file() {
        let temp = tempfile::tempdir().expect("temp journal");
        let (outbound, receiver) = mpsc::channel();
        let dispatcher = SenseDispatcher::new(temp.path().to_path_buf(), false, false, outbound);
        dispatcher.handle(&observing("one", "120000_1"));
        let event = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(event.fields["note"], "no_engine");
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
        let (sender, failed_receiver) = mpsc::channel();
        drop(failed_receiver);
        dispatcher
            .pools
            .get("transcribe")
            .expect("transcribe pool")
            .lock()
            .expect("pool")
            .senders = vec![sender];
        let mut message = observing("one", "120000_1");
        message.extra.insert("files".into(), json!(["audio.flac"]));
        dispatcher.handle(&message);
        let observed = receiver
            .try_iter()
            .find(|event| event.event == "observed")
            .expect("observed");
        assert_eq!(observed.fields["error"], true);
        assert!(
            observed.fields["errors"][0]
                .as_str()
                .expect("error")
                .contains("transcribe pool unavailable")
        );
        let state = dispatcher.state.lock().expect("state");
        assert!(state.segments.is_empty());
        assert!(state.pending_files.is_empty());
    }

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
        let (sender, failed_receiver) = mpsc::channel();
        drop(failed_receiver);
        dispatcher
            .pools
            .get("transcribe")
            .expect("transcribe pool")
            .lock()
            .expect("pool")
            .senders = vec![sender];
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

    #[test]
    fn process_tree_not_reaped_becomes_a_path_free_segment_error_detail() {
        let detail = termination_detail(
            "describe",
            TerminationError::ProcessTreeNotReaped {
                reason: "survived_sigkill",
                survivors: vec![Descendant {
                    pid: 99,
                    pgid: Some(99),
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
                observer: None,
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
    fn idle_status_tick_resets_the_rolling_beacon_error_count() {
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
        assert_eq!(status.fields["recent_error_count"], 0);
        assert!(status.fields["last_successful_sync"].is_i64());
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
                observer: None,
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
                observer: None,
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
                observer: None,
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
                observer: None,
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
                observer: None,
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
}
