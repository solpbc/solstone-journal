// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned processing using the maintained discovery, Sense, think and schedule owners.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_backup_web::BackupWebDeps;
use solstone_core_cortex::{CallableCortex, CallableRoots};
use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, WaitForUsesReport,
};
use solstone_core_depict::WireClient;
use solstone_core_describe::session::{DescribeSession, DescribeSessionFactory};
use solstone_core_generate::{
    ClientError, GenerateExecutor, GenerateRequest, GenerateResponse, SessionCloseError,
    SessionCompletion, SessionFailure, SessionFailureReason, SessionReceiveError,
    SessionSubmitError,
};
use solstone_core_journal_config::work_priority::{WorkBand, WorkPriority};
use solstone_core_journal_io::JournalRoot;
use solstone_core_sense::{SenseExecutor, SenseJobResult};
use solstone_core_system::cap::DefaultCapResolver;
use solstone_core_system::process::ChildLaunchContext;
use solstone_core_system::queue::{
    CallableQueueWorker, CallableTaskContext, ProcessState, ProcessStateProbe, TaskQueue,
    TaskQueueEvent, TaskQueueEventSink, TaskQueueOptions,
};

use solstone_core_system::request::{BusTaskRequest, ExecutionRequest, ScheduledRequest, TaskArgv};
use solstone_core_system::schedule::{
    CheckReport, ScheduleEngine, ScheduleError, ScheduleNow, ScheduleSubmissionSink,
};
use solstone_core_system::status_wire::{
    ServiceCandidate, SupervisorStatusWireInput, project_supervisor_status,
};
use solstone_core_think_cli::planner::{
    FlushState, PendingFlush, WorkDiscovery, check_activity_settle, check_segment_flush,
    day_still_closing,
};
use solstone_core_think_cli::{CortexBoundary, DispatchFailure, IndexBoundary, ThinkContext};

/// Persist the explicit owner hold through the bound journal even before the
/// foreground host starts. Generic lifecycle cancellation never calls this.
pub fn persist_owner_stop(root: &JournalRoot) -> Result<(), String> {
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    solstone_core_system::work_intent::stop_rebuilds(root.canonical_path())?;
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())
}

static HOST_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(all(test, feature = "full-tests"))]
static TEST_LOCK: Mutex<()> = Mutex::new(());
#[cfg(all(test, feature = "full-tests"))]
static DESCRIBE_CANCEL_AT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(all(test, feature = "full-tests"))]
static DESCRIBE_POLLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(all(test, feature = "full-tests"))]
static DESCRIBE_TEST_LOCK: Mutex<()> = Mutex::new(());
#[cfg(all(test, feature = "full-tests"))]
static DESCRIBE_AFTER_ENTRY: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);
#[cfg(all(test, feature = "full-tests"))]
static TEST_DEPICT_DETECTION: Mutex<Option<Value>> = Mutex::new(None);
type Checkpoint = Arc<dyn Fn() -> bool + Send + Sync>;
const UNCOMPLETED_HOLD_EXIT: i32 = 4;

#[path = "scheduled_commands.rs"]
mod scheduled_commands;

const MAINTENANCE_ROUTINE_IDS: &[&str] = &[
    "speakers:discover-voices",
    "speakers:candidate-pair-suggestions",
    "speakers:name-variants",
    "speakers:consolidate-pool",
    "speakers:backfill-pool",
    "backup:run",
    "backup:verify",
    "backup:prune",
    "health:mark-raw",
    "health:prune-logs",
];

struct NoProcesses;
impl ProcessStateProbe for NoProcesses {
    fn state(&self, _: u32) -> ProcessState {
        panic!("a callable queue must not launch a child process")
    }
}

pub struct ProductionGenerateExecutor {
    root: Arc<JournalRoot>,
    cancelled: Checkpoint,
    priority: WorkPriority,
}
impl GenerateExecutor for ProductionGenerateExecutor {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        let cancellation = solstone_core_generate::Cancellation::from_fn(self.cancelled.as_ref());
        let runtime = solstone_core_generate_wire::EndpointRuntime::default();
        solstone_core_generate_wire::respond_with_context(
            request,
            &runtime,
            self.root.canonical_path(),
            |_| None,
            &solstone_core_generate_wire::GenerateCallContext {
                priority: self.priority,
                cancellation: &cancellation,
            },
        )
        .map_err(ClientError::Decode)
    }
}

#[derive(Clone)]
struct HostGenerator {
    root: Arc<JournalRoot>,
    injected: Option<Arc<dyn GenerateExecutor + Send + Sync>>,
}
impl HostGenerator {
    fn executor(
        &self,
        cancelled: Checkpoint,
        priority: WorkPriority,
    ) -> Arc<dyn GenerateExecutor + Send + Sync> {
        self.injected.clone().unwrap_or_else(|| {
            Arc::new(ProductionGenerateExecutor {
                root: Arc::clone(&self.root),
                cancelled,
                priority,
            })
        })
    }
}

struct HostWireClient<'a> {
    generator: &'a HostGenerator,
    checkpoint: &'a (dyn Fn() -> bool + Send + Sync),
    priority: WorkPriority,
}
impl WireClient for HostWireClient<'_> {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        if (self.checkpoint)() || !media_work_permitted(self.generator, self.priority) {
            return Ok(GenerateResponse::Cancelled);
        }
        if let Some(executor) = self.generator.injected.as_ref() {
            return executor.execute(request);
        }
        let cancellation = solstone_core_generate::Cancellation::from_fn(self.checkpoint);
        solstone_core_generate_wire::respond_with_context(
            request,
            &solstone_core_generate_wire::EndpointRuntime::default(),
            self.generator.root.canonical_path(),
            |_| None,
            &solstone_core_generate_wire::GenerateCallContext {
                priority: self.priority,
                cancellation: &cancellation,
            },
        )
        .map_err(ClientError::Decode)
    }
}

fn media_work_permitted(generator: &HostGenerator, priority: WorkPriority) -> bool {
    solstone_core_system::work_policy::WorkPolicy::read(generator.root.canonical_path())
        .is_ok_and(|policy| policy.permits(priority.band))
}

#[cfg(test)]
struct NoDetector;
#[cfg(test)]
impl solstone_core_depict::Detector for NoDetector {
    fn detect(&self, _: &[u8]) -> Result<Option<Value>, String> {
        Ok(None)
    }
}

#[cfg(all(test, feature = "full-tests"))]
struct FixedDetector(Value);
#[cfg(all(test, feature = "full-tests"))]
impl solstone_core_depict::Detector for FixedDetector {
    fn detect(&self, _: &[u8]) -> Result<Option<Value>, String> {
        Ok(Some(self.0.clone()))
    }
}

fn stamp_missing_id(request_id: Option<&str>, response: &mut GenerateResponse) {
    let Some(request_id) = request_id else {
        return;
    };
    match response {
        GenerateResponse::Generated(generated) if generated.id.is_none() => {
            generated.id = Some(request_id.to_owned());
        }
        GenerateResponse::Refused(refused) if refused.id.is_none() => {
            refused.id = Some(request_id.to_owned());
        }
        _ => {}
    }
}

struct InProcessDescribeSession<'a> {
    generator: &'a HostGenerator,
    checkpoint: &'a (dyn Fn() -> bool + Send + Sync),
    priority: WorkPriority,
    cap: usize,
    closed: AtomicBool,
    queue: Mutex<VecDeque<GenerateRequest>>,
}

impl DescribeSession for InProcessDescribeSession<'_> {
    fn submit(&self, request: GenerateRequest) -> Result<(), SessionSubmitError> {
        let mut queue = self
            .queue
            .lock()
            .map_err(|_| SessionSubmitError::WriterUnavailable)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(SessionSubmitError::Closed);
        }
        if queue.len() >= self.cap {
            return Err(SessionSubmitError::WriterUnavailable);
        }
        queue.push_back(request);
        Ok(())
    }

    fn recv_timeout(&self, _timeout: Duration) -> Result<SessionCompletion, SessionReceiveError> {
        let request = {
            let mut queue = self
                .queue
                .lock()
                .map_err(|_| SessionReceiveError::Disconnected)?;
            queue.pop_front()
        };
        let Some(request) = request else {
            return Err(if self.closed.load(Ordering::Acquire) {
                SessionReceiveError::Disconnected
            } else {
                SessionReceiveError::TimedOut
            });
        };
        if self.closed.load(Ordering::Acquire)
            || (self.checkpoint)()
            || !media_work_permitted(self.generator, self.priority)
        {
            return Ok(SessionCompletion::Response(GenerateResponse::Cancelled));
        }
        let executed = if let Some(executor) = self.generator.injected.as_ref() {
            executor.execute(&request)
        } else {
            let cancellation = solstone_core_generate::Cancellation::from_fn(self.checkpoint);
            solstone_core_generate_wire::respond_with_context(
                &request,
                &solstone_core_generate_wire::EndpointRuntime::default(),
                self.generator.root.canonical_path(),
                |_| None,
                &solstone_core_generate_wire::GenerateCallContext {
                    priority: self.priority,
                    cancellation: &cancellation,
                },
            )
            .map_err(ClientError::Decode)
        };
        match executed {
            Ok(GenerateResponse::Cancelled) => {
                Ok(SessionCompletion::Response(GenerateResponse::Cancelled))
            }
            Ok(mut response) => {
                stamp_missing_id(request.id.as_deref(), &mut response);
                Ok(SessionCompletion::Response(response))
            }
            Err(error) => Ok(SessionCompletion::Failure(SessionFailure {
                id: request.id.unwrap_or_default(),
                reason: SessionFailureReason::Desynchronized(error.to_string()),
                retryable: false,
                blocking: true,
            })),
        }
    }

    fn close(&self) -> Result<(), SessionCloseError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Err(SessionCloseError::AlreadyClosed);
        }
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
        Ok(())
    }
}

struct InProcessDescribeFactory<'a> {
    generator: &'a HostGenerator,
    checkpoint: &'a (dyn Fn() -> bool + Send + Sync),
    priority: WorkPriority,
}

impl DescribeSessionFactory for InProcessDescribeFactory<'_> {
    fn spawn(
        &self,
        max_in_flight: usize,
        _explicit_journal: Option<&Path>,
    ) -> Result<Box<dyn DescribeSession + '_>, solstone_core_generate::SessionLaunchError> {
        Ok(Box::new(InProcessDescribeSession {
            generator: self.generator,
            checkpoint: self.checkpoint,
            priority: self.priority,
            cap: max_in_flight.saturating_mul(2).max(1),
            closed: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
        }))
    }
}

fn map_depict(
    result: Result<solstone_core_depict::RunOutcome, solstone_core_depict::DepictError>,
) -> SenseJobResult {
    match result {
        Ok(solstone_core_depict::RunOutcome::Written) => SenseJobResult::Finished(0),
        Ok(_) => SenseJobResult::Hold,
        Err(solstone_core_depict::DepictError::Cancelled) => SenseJobResult::Hold,
        Err(_) => SenseJobResult::Finished(1),
    }
}

fn depict_job(
    journal: &Path,
    payload: &Path,
    input: &Path,
    wire: &HostWireClient<'_>,
    checkpoint: &dyn Fn() -> bool,
    attempt: Option<&solstone_core_system::work_retry::Attempt>,
) -> SenseJobResult {
    #[cfg(all(test, feature = "full-tests"))]
    if let Some(value) = TEST_DEPICT_DETECTION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
    {
        return map_depict(solstone_core_depict::run_with_clients_and_checkpoint(
            input,
            false,
            wire,
            &FixedDetector(value),
            checkpoint,
            attempt,
        ));
    }
    let detector = crate::rfdetr::RfdetrDetector::from_payload(journal, payload);
    map_depict(solstone_core_depict::run_with_clients_and_checkpoint(
        input, false, wire, &detector, checkpoint, attempt,
    ))
}

struct HostIndexBoundary {
    cancelled: Checkpoint,
}
impl IndexBoundary for HostIndexBoundary {
    fn drain_pending(
        &self,
        journal: &Path,
        force: bool,
        budget: Duration,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        let report = solstone_core_indexer_store::todo::drain_with_checkpoint(
            journal,
            solstone_core_indexer_store::todo::DrainOptions { force, budget },
            checkpoint,
        )
        .map_err(|error| error.to_string())?;
        if report.timed_out || report.failed != 0 {
            Err("search updates are still pending".into())
        } else {
            Ok(())
        }
    }
    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
        if (self.cancelled)() {
            return Err("thinking cancelled".into());
        }
        solstone_core_indexer_store::todo::enqueue_path(journal, path).map_err(|e| e.to_string())
    }
}

/// The talents the cortex boundary is running now, for the status Health reads.
#[derive(Clone, Default)]
struct RunningUses(Arc<Mutex<BTreeMap<String, RunningUse>>>);
struct RunningUse {
    name: String,
    provider: Value,
    started: Instant,
}
struct RunningUseGuard {
    uses: RunningUses,
    use_id: String,
}
impl RunningUses {
    fn begin(&self, use_id: &str, request: &CortexRequest) -> RunningUseGuard {
        let provider = request
            .config
            .get("provider")
            .cloned()
            .unwrap_or_else(|| Value::String("unknown".into()));
        if let Ok(mut running) = self.0.lock() {
            running.insert(
                use_id.to_owned(),
                RunningUse {
                    name: request.name.clone(),
                    provider,
                    started: Instant::now(),
                },
            );
        }
        RunningUseGuard {
            uses: self.clone(),
            use_id: use_id.to_owned(),
        }
    }
    /// The cortex status fields. Talents run in-process here, so there is no
    /// separate cortex queue.
    fn status(&self) -> Option<Map<String, Value>> {
        let running = self.0.lock().ok()?;
        let uses = running
            .iter()
            .map(|(use_id, running)| {
                json!({
                    "use_id": use_id,
                    "name": running.name,
                    "provider": running.provider,
                    "elapsed_seconds": running.started.elapsed().as_secs(),
                })
            })
            .collect::<Vec<_>>();
        Some(Map::from_iter([
            ("running_uses".into(), json!(running.len())),
            ("uses".into(), Value::Array(uses)),
            ("queue_depth".into(), json!(0)),
        ]))
    }
}
impl Drop for RunningUseGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.uses.0.lock() {
            running.remove(&self.use_id);
        }
    }
}

const STATUS_INTERVAL: Duration = Duration::from_secs(5);

/// Publish what this runtime holds, in the shape Health reads from the desktop
/// supervisor and cortex. On a phone the runtime is the journal's one service:
/// it runs the task queue, the schedules and every talent in-process, so it is
/// the only service row. Called while the runtime runs, never after it stops
/// or fails, so Health's own staleness rule clears the panels then.
fn publish_status(
    journal: &Path,
    queue: &TaskQueue,
    engine: &Mutex<ScheduleEngine>,
    running: &RunningUses,
    started: Instant,
) {
    let Ok(schedules) = engine.lock().map(|engine| {
        engine.collect_status(ScheduleNow::in_zone(
            Utc::now(),
            solstone_core_journal_config::owner_zone(journal),
        ))
    }) else {
        return;
    };
    let Some(cortex) = running.status() else {
        return;
    };
    let supervisor = project_supervisor_status(SupervisorStatusWireInput {
        services: vec![ServiceCandidate::SupervisorSelf {
            reference: "journal".into(),
            pid: std::process::id(),
            uptime_seconds: started.elapsed().as_secs(),
        }],
        crashed: Vec::new(),
        queue: queue.collect_status_snapshot(Instant::now()),
        stale_heartbeats: Vec::new(),
        schedules,
        callosum_clients: 0,
        sense_pending_queue_depth: None,
        sense_pending_age_ms: None,
        sense_pending_received: false,
    });
    for (tract, fields) in [("supervisor", supervisor), ("cortex", cortex)] {
        let envelope = solstone_core_callosum::CallosumEnvelope {
            tract: tract.into(),
            event: "status".into(),
            ts: Some(now_ms()),
            extra: fields,
        };
        if let Ok(mut line) = serde_json::to_string(&envelope) {
            line.push('\n');
            let _ = solstone_core_callosum::CallosumOneShotSender::new(
                journal.join("health/callosum.sock"),
                Duration::from_secs(1),
            )
            .send_line(&line);
        }
    }
}

struct HostCortexBoundary {
    journal: PathBuf,
    talent_root: PathBuf,
    apps_root: PathBuf,
    templates_dir: PathBuf,
    generator: Arc<dyn GenerateExecutor + Send + Sync>,
    client: CortexRequestClient,
    cancelled: Checkpoint,
    owner: Arc<CallableCortex>,
    running: RunningUses,
}
impl CortexBoundary for HostCortexBoundary {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, DispatchFailure> {
        self.dispatch_prepared(runtime, request, None, &mut |_| Ok(()))
    }
    fn dispatch_prepared(
        &self,
        _: &tokio::runtime::Runtime,
        request: &CortexRequest,
        reserved: Option<&str>,
        prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
    ) -> Result<String, DispatchFailure> {
        if (self.cancelled)() {
            return Err(DispatchFailure::Unavailable);
        }
        let now_ms = now_ms();
        let use_id = match reserved {
            Some(id) => id.to_owned(),
            None => {
                solstone_core_journal_io::cortex_use::allocate_cortex_use_id(&self.journal, now_ms)
                    .map_err(|_| DispatchFailure::Unavailable)?
                    .to_string()
            }
        };
        prepare(&use_id).map_err(|_| DispatchFailure::Unavailable)?;
        if (self.cancelled)() {
            return Err(DispatchFailure::Unavailable);
        }
        let mut extra = Map::new();
        extra.insert("prompt".into(), Value::String(request.prompt.clone()));
        extra.extend(request.config.clone());
        extra.insert("name".into(), Value::String(request.name.clone()));
        extra.insert("use_id".into(), Value::String(use_id.clone()));
        let _running = self.running.begin(&use_id, request);
        let outcome = self
            .owner
            .execute(
                CallableRoots {
                    talent_root: &self.talent_root,
                    apps_root: &self.apps_root,
                    templates_dir: &self.templates_dir,
                },
                extra,
                self.generator.as_ref(),
                now_ms,
                self.cancelled.as_ref(),
            )
            .map_err(|_| DispatchFailure::Unavailable)?;
        if !outcome.completed || (self.cancelled)() {
            return Err(DispatchFailure::Unavailable);
        }
        Ok(use_id)
    }
    fn wait(
        &self,
        _: &tokio::runtime::Runtime,
        ids: &[String],
        _: Option<Duration>,
    ) -> Result<WaitForUsesReport, String> {
        if (self.cancelled)() {
            return Err("thinking cancelled".into());
        }
        self.client
            .wait_for_uses_from_disk(ids)
            .map_err(|e| format!("cortex wait failed: {e:?}"))
    }
}

struct HostScheduleSink {
    queue: Arc<TaskQueue>,
}
impl ScheduleSubmissionSink for HostScheduleSink {
    fn submit(&self, request: ScheduledRequest) -> bool {
        !matches!(
            self.queue.submit(ExecutionRequest::Scheduled(request)),
            solstone_core_system::queue::SubmitOutcome::Rejected
        )
    }
}
struct HostQueueSink {
    daily_observation: Option<Arc<crate::daily_queue_observation::DailyQueueTap>>,
    queue: Arc<OnceLock<Weak<TaskQueue>>>,
    schedule_engine: Arc<Mutex<ScheduleEngine>>,
    flush: Arc<Mutex<FlushState>>,
    cancelled: Checkpoint,
}
impl TaskQueueEventSink for HostQueueSink {
    fn emit(&self, event: TaskQueueEvent) {
        if (self.cancelled)() {
            return;
        }
        if let Some(tap) = &self.daily_observation {
            tap.record(&event);
        }
        if let TaskQueueEvent::Stopped {
            reference,
            exit_code,
            command,
            ..
        } = event
        {
            if exit_code == 0
                && let Some(args) = admit_argv(&command)
                && !args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "--flush" | "--settle"))
            {
                let value = |flag: &str| {
                    args.windows(2)
                        .find(|pair| pair[0] == flag)
                        .map(|pair| pair[1].clone())
                };
                if let (Some(day), Some(segment)) = (value("--day"), value("--segment"))
                    && let Ok(mut flush) = self.flush.lock()
                    && !(self.cancelled)()
                {
                    let stream = value("--stream");
                    if flush
                        .pending
                        .get(&stream)
                        .is_none_or(|pending| (&day, &segment) >= (&pending.day, &pending.segment))
                    {
                        flush.pending.insert(
                            stream,
                            PendingFlush {
                                last_segment_ts: Instant::now(),
                                day,
                                segment,
                            },
                        );
                    }
                }
            }
            if (self.cancelled)() {
                return;
            }
            let Some(queue) = self.queue.get().and_then(Weak::upgrade) else {
                log::warn!("foreground schedule stopped before its queue was installed");
                return;
            };
            let Some(name) = queue
                .history()
                .into_iter()
                .rev()
                .find(|record| record.reference == reference)
                .and_then(|record| record.scheduler_name)
            else {
                return;
            };
            if let Ok(engine) = self.schedule_engine.lock() {
                let checkpoint = || {
                    if (self.cancelled)() {
                        Err("cancelled".to_owned())
                    } else {
                        Ok(())
                    }
                };
                if checkpoint().is_err() {
                    return;
                }
                let ended_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0., |d| d.as_secs_f64());
                let status = if exit_code == 0 {
                    "success"
                } else if exit_code == UNCOMPLETED_HOLD_EXIT {
                    "uncompleted-hold"
                } else {
                    "failed"
                };
                let _ = engine.record_completion_checked(
                    &name,
                    ended_at,
                    status,
                    &reference,
                    &checkpoint,
                );
            }
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
fn strip_launcher(args: &[String]) -> Vec<String> {
    let mut args = args.to_vec();
    while args
        .first()
        .is_some_and(|arg| arg == "solstone" || arg == "journal")
    {
        args.remove(0);
    }
    args
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn think_admitted(args: &[String]) -> bool {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--refresh" | "--segments"))
    {
        return false;
    }
    if has_flag(args, "--from-scratch") {
        return flag_value(args, "--day").is_some()
            && flag_value(args, "--work-intent").is_some_and(|id| !id.is_empty())
            && !has_flag(args, "--weekly")
            && !has_flag(args, "--daily-only");
    }
    if has_flag(args, "--weekly") {
        return true;
    }
    if has_flag(args, "--daily-only") {
        return flag_value(args, "--day").is_some();
    }
    has_flag(args, "--cadence")
        || (has_flag(args, "--settle") && flag_value(args, "--stream").is_some())
        || (has_flag(args, "--segment")
            && flag_value(args, "--day").is_some()
            && flag_value(args, "--stream").is_some())
        || (has_flag(args, "--activity")
            && flag_value(args, "--day").is_some()
            && flag_value(args, "--facet").is_some())
        || (flag_value(args, "--retry-unit").is_some_and(|id| !id.is_empty())
            && flag_value(args, "--retry-cause").is_some()
            && flag_value(args, "--day").is_some())
}

fn exact_command(args: &[String]) -> bool {
    match args {
        [command] if command == "heartbeat" || command == "facet-candidates" => true,
        [command, verb] if command == "brain" && verb == "refresh" => true,
        [command, flag]
            if command == "indexer"
                && ["--rebuild-edges", "--rescan", "drain"].contains(&flag.as_str()) =>
        {
            true
        }
        [command, verb, id]
            if command == "maintenance"
                && verb == "run"
                && MAINTENANCE_ROUTINE_IDS.contains(&id.as_str()) =>
        {
            true
        }
        _ => false,
    }
}

fn admit_argv(args: &[String]) -> Option<Vec<String>> {
    let args = strip_launcher(args);
    let [command, rest @ ..] = args.as_slice() else {
        return None;
    };
    if command != "think" || !think_admitted(rest) {
        return None;
    }
    Some(rest.to_vec())
}

fn admit_command(journal: &Path, raw: &[String]) -> bool {
    let args = strip_launcher(raw);
    let [command, rest @ ..] = args.as_slice() else {
        return false;
    };
    if command == "think" {
        if !think_admitted(rest) {
            return false;
        }
        if let Some(id) = flag_value(rest, "--retry-unit") {
            return matches!(
                solstone_core_system::work_retry::read(journal, id),
                Ok(Some(record)) if matches!(
                    record.unit,
                    solstone_core_system::work_retry::Unit::Raw { .. }
                        | solstone_core_system::work_retry::Unit::Segment { .. }
                        | solstone_core_system::work_retry::Unit::Activity { .. }
                        | solstone_core_system::work_retry::Unit::Daily { .. }
                )
            );
        }
        return true;
    }
    exact_command(&args)
}

#[cfg(feature = "audio")]
#[derive(Clone)]
struct AudioConfig {
    worker: Arc<crate::audio::AudioWorker>,
}

struct HostSenseExecutor {
    journal: PathBuf,
    payload: PathBuf,
    generator: HostGenerator,
    admitted_priority: Option<WorkPriority>,
    cancelled: Checkpoint,
    failed: Arc<AtomicBool>,
    #[cfg(feature = "audio")]
    audio_state: Arc<Mutex<Option<AudioConfig>>>,
}
impl SenseExecutor for HostSenseExecutor {
    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
    fn execute(
        &self,
        handler: &str,
        input: &Path,
        context: &solstone_core_sense::work::SegmentContext,
        attempt: Option<&solstone_core_system::work_retry::Attempt>,
        stop: &AtomicBool,
    ) -> SenseJobResult {
        let interrupted = || (self.cancelled)() || stop.load(Ordering::Acquire);
        let accepted = if matches!(handler, "depict" | "describe") {
            match crate::custody::AcceptedInput::capture(&self.journal, input, &interrupted) {
                Ok(Some(accepted)) => Some(accepted),
                _ => return SenseJobResult::Hold,
            }
        } else {
            None
        };
        let checkpoint = || {
            #[cfg(all(test, feature = "full-tests"))]
            if handler == "describe" {
                let n = DESCRIBE_POLLS.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 2
                    && let Some(hook) = DESCRIBE_AFTER_ENTRY
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .take()
                {
                    hook();
                }
                let fire = DESCRIBE_CANCEL_AT.load(Ordering::SeqCst);
                if fire != 0 && n >= fire {
                    return true;
                }
            }
            interrupted()
                || accepted
                    .as_ref()
                    .is_some_and(|accepted| !accepted.still_current(input))
        };
        if checkpoint() || !solstone_core_system::processing::processing_allowed(&self.journal) {
            return SenseJobResult::Hold;
        }
        let mut priority = self.admitted_priority.unwrap_or_else(|| {
            solstone_core_system::priority::classify(
                &[
                    "journal".into(),
                    "think".into(),
                    "--day".into(),
                    context.key.day.clone(),
                    "--segment".into(),
                    context.key.segment.clone(),
                ],
                Utc::now()
                    .with_timezone(&solstone_core_journal_config::owner_zone(&self.journal))
                    .date_naive(),
            )
        });
        if self.admitted_priority.is_none()
            && let Some(attempt) = attempt
        {
            priority.band = if attempt.cause.starts_with("owner:") {
                WorkBand::Waiting
            } else {
                WorkBand::Completing
            };
        }
        if matches!(handler, "depict" | "describe")
            && !solstone_core_system::work_policy::WorkPolicy::read(&self.journal)
                .is_ok_and(|policy| policy.permits(priority.band))
        {
            return SenseJobResult::Hold;
        }
        let result = match handler {
            "depict" => depict_job(
                &self.journal,
                &self.payload,
                input,
                &HostWireClient {
                    generator: &self.generator,
                    checkpoint: &checkpoint,
                    priority,
                },
                &checkpoint,
                attempt,
            ),
            "describe" => self.describe_job(input, attempt, priority, &checkpoint),
            "transcribe" => {
                #[cfg(feature = "audio")]
                {
                    let config = self.audio_state.lock().unwrap().clone();
                    match config {
                        Some(config) => {
                            match config
                                .worker
                                .process_host_input(input, attempt, &checkpoint)
                            {
                                Ok(true) => SenseJobResult::Finished(0),
                                Ok(false) => SenseJobResult::Hold,
                                Err(_) => SenseJobResult::Finished(1),
                            }
                        }
                        None => SenseJobResult::Hold,
                    }
                }
                #[cfg(not(feature = "audio"))]
                SenseJobResult::Hold
            }
            _ => SenseJobResult::Hold,
        };
        if checkpoint() {
            SenseJobResult::Hold
        } else {
            result
        }
    }
}

impl HostSenseExecutor {
    fn describe_job(
        &self,
        input: &Path,
        attempt: Option<&solstone_core_system::work_retry::Attempt>,
        priority: WorkPriority,
        checkpoint: &(dyn Fn() -> bool + Send + Sync),
    ) -> SenseJobResult {
        let settings = match solstone_core_describe::read_config(&self.journal) {
            Ok(settings) => settings,
            Err(_) => return SenseJobResult::Finished(1),
        };
        let sense_config = solstone_core_sense::config::read_config(&self.journal);
        let workers = solstone_core_sense::config::resolve_concurrency(&sense_config, "describe");
        let jobs = solstone_core_sense::config::describe_per_proc_jobs(
            &sense_config,
            workers,
            &self.journal,
        );
        let factory = InProcessDescribeFactory {
            generator: &self.generator,
            checkpoint,
            priority,
        };
        let mut ocr = crate::screen_ocr::ScreenOcrFactory::new(&self.payload);
        let detector = crate::rfdetr::RfdetrDetector::from_payload(&self.journal, &self.payload);
        match solstone_core_describe::pipeline::run_with_clients(
            solstone_core_describe::pipeline::DescribeOptions {
                video: input,
                journal: &self.journal,
                explicit_journal: Some(&self.journal),
                jobs,
                redo: false,
                config: settings.winnow,
                redact_rules: settings.redact_rules,
                max_extractions: settings.max_extractions,
                category_overrides: settings.category_overrides,
            },
            &factory,
            &detector,
            &mut ocr,
            checkpoint,
            attempt,
        ) {
            Ok(()) => {
                if solstone_core_sense::unfinished_output(input, "describe").is_none() {
                    SenseJobResult::Finished(0)
                } else {
                    SenseJobResult::Finished(1)
                }
            }
            Err(solstone_core_describe::pipeline::RunError::Cancelled) => SenseJobResult::Hold,
            Err(_) => SenseJobResult::Finished(1),
        }
    }
}

struct HostedCommand {
    rebuild_recovery: bool,
    journal: PathBuf,
    payload: PathBuf,
    root: Arc<JournalRoot>,
    talent_root: PathBuf,
    apps_root: PathBuf,
    templates_dir: PathBuf,
    generator: HostGenerator,
    cancelled: Checkpoint,
    cortex_owner: Arc<CallableCortex>,
    running: RunningUses,
    failed: Arc<AtomicBool>,
    backup: BackupWebDeps,
    #[cfg(feature = "audio")]
    audio_state: Arc<Mutex<Option<AudioConfig>>>,
}
pub struct ForegroundProcessingHost {
    daily_observation: Option<Arc<crate::daily_queue_observation::DailyQueueTap>>,
    journal: PathBuf,
    payload: PathBuf,
    /// Held so the canonical root stays bound until the host drops.
    #[allow(dead_code)]
    root: Arc<JournalRoot>,
    queue: Arc<TaskQueue>,
    schedule_engine: Arc<Mutex<ScheduleEngine>>,
    flush: Arc<Mutex<FlushState>>,
    discovery: Mutex<Option<WorkDiscovery>>,
    sense_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    schedule_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    running: RunningUses,
    stop: Arc<AtomicBool>,
    cancelled: Checkpoint,
    failed: Arc<AtomicBool>,
    generator: HostGenerator,
    #[cfg(test)]
    worker: CallableQueueWorker,
    #[cfg(feature = "audio")]
    audio_state: Arc<Mutex<Option<AudioConfig>>>,
}

impl ForegroundProcessingHost {
    pub fn open(journal: &Path, payload: &Path, backup: BackupWebDeps) -> Result<Self, String> {
        let now = ScheduleNow::in_zone(
            Utc::now(),
            solstone_core_journal_config::owner_zone(journal),
        );
        Self::open_internal(journal, payload, now, None, backup)
    }
    pub fn open_at(
        journal: &Path,
        payload: &Path,
        now: ScheduleNow,
        backup: BackupWebDeps,
    ) -> Result<Self, String> {
        Self::open_internal(journal, payload, now, None, backup)
    }
    pub fn open_with_executor(
        journal: &Path,
        payload: &Path,
        now: ScheduleNow,
        executor: Arc<dyn GenerateExecutor + Send + Sync>,
        backup: BackupWebDeps,
    ) -> Result<Self, String> {
        Self::open_internal(journal, payload, now, Some(executor), backup)
    }
    fn open_internal(
        journal: &Path,
        payload: &Path,
        now: ScheduleNow,
        executor: Option<Arc<dyn GenerateExecutor + Send + Sync>>,
        backup: BackupWebDeps,
    ) -> Result<Self, String> {
        let root = Arc::new(JournalRoot::open(journal).map_err(|e| e.to_string())?);
        Self::open_bound(
            root,
            payload,
            now,
            executor,
            Arc::new(|| false),
            Arc::new(AtomicBool::new(false)),
            backup,
            false,
        )
    }
    pub fn open_with_cancellation(
        root: Arc<JournalRoot>,
        payload: &Path,
        external: Checkpoint,
        stop: Arc<AtomicBool>,
        backup: BackupWebDeps,
    ) -> Result<Self, String> {
        let now = ScheduleNow::in_zone(
            Utc::now(),
            solstone_core_journal_config::owner_zone(root.canonical_path()),
        );
        Self::open_bound(root, payload, now, None, external, stop, backup, false)
    }
    pub fn open_with_rebuild_recovery(
        root: Arc<JournalRoot>,
        payload: &Path,
        external: Checkpoint,
        stop: Arc<AtomicBool>,
        backup: BackupWebDeps,
    ) -> Result<Self, String> {
        let now = ScheduleNow::in_zone(
            Utc::now(),
            solstone_core_journal_config::owner_zone(root.canonical_path()),
        );
        Self::open_bound(root, payload, now, None, external, stop, backup, true)
    }
    /// Install a development-only event observation before any queue can start.
    /// Its emissions never reserve work or authorize a processing-mode change.
    pub fn open_with_rebuild_recovery_observed(
        root: Arc<JournalRoot>,
        payload: &Path,
        external: Checkpoint,
        stop: Arc<AtomicBool>,
        backup: BackupWebDeps,
    ) -> Result<Self, String> {
        let now = ScheduleNow::in_zone(
            Utc::now(),
            solstone_core_journal_config::owner_zone(root.canonical_path()),
        );
        Self::open_bound_observed(root, payload, now, None, external, stop, backup, true, true)
    }

    /// Historical emissions and a separate current owner read, never a reservation.
    pub fn daily_queue_observation(&self) -> Result<Option<Value>, &'static str> {
        let Some(tap) = &self.daily_observation else {
            return Ok(None);
        };
        if (self.cancelled)() {
            tap.close();
            return Err("observer_stopped");
        }
        let mut observation = match tap.read() {
            Ok(value) => value,
            // A stale emission stays unavailable; it must not hide the separate
            // current read used to corroborate later request-owned progress.
            Err("observer_emission_stale") => {
                let (root, host) = tap.binding()?;
                json!({"native_root":root,"host_instance":host,
                    "historical_error":"observer_emission_stale","authorizes_release":false})
            }
            Err(error) => return Err(error),
        };
        let current = self.queue.try_read_daily()?;
        // No config/root I/O or serialization runs under the queue state lock.
        let policy_now = Utc::now();
        let (settings, gate) =
            solstone_core_system::processing::policy_at(&self.journal, policy_now)
                .map_err(|_| "observer_policy_unavailable")?;
        if (self.cancelled)() {
            tap.close();
            return Err("observer_stopped");
        }
        let (root, host) = tap.binding()?;
        if observation["native_root"] != root || observation["host_instance"] != host {
            return Err("observer_binding_changed");
        }
        let age = crate::daily_queue_observation::live_read_age(
            current.read_started,
            current.read_finished,
            Instant::now(),
        )?;
        observation["current"] = json!({
            "native_root":root,"host_instance":host,"read_start_age_us":age,
            "ready":current.ready,"shutdown":current.shutdown,"global_hold":current.global_hold,
            "held":current.held,"running_or_reserved":current.running_or_reserved,
            "slot":current.slot,"index_conflict":current.index_conflict,"queued":current.queued,
            "policy_checked_at":policy_now.to_rfc3339(),
            "policy":{"deferred":settings.deferred,"gate_open":gate.open,
                "time_window":{"enabled":gate.time_window.enabled,"available":gate.time_window.available,
                    "open":gate.time_window.open,"start":settings.gate.time_window.start.to_string(),
                    "end":settings.gate.time_window.end.to_string()},
                "display_powersave":{"enabled":gate.display_powersave.enabled,
                    "available":gate.display_powersave.available,"open":gate.display_powersave.open}},
            "atomic_with_policy_or_settings":false,"authorizes_release":false
        });
        // Check age again after bounded serialization; completion never resets it.
        if (self.cancelled)() {
            tap.close();
            return Err("observer_stopped");
        }
        let final_age = crate::daily_queue_observation::live_read_age(
            current.read_started,
            current.read_finished,
            Instant::now(),
        )?;
        observation["current"]["read_start_age_us"] = serde_json::json!(final_age);
        Ok(Some(observation))
    }
    #[allow(clippy::too_many_arguments)]
    fn open_bound(
        root: Arc<JournalRoot>,
        payload: &Path,
        now: ScheduleNow,
        executor: Option<Arc<dyn GenerateExecutor + Send + Sync>>,
        external: Checkpoint,
        stop: Arc<AtomicBool>,
        backup: BackupWebDeps,
        rebuild_recovery: bool,
    ) -> Result<Self, String> {
        Self::open_bound_observed(
            root,
            payload,
            now,
            executor,
            external,
            stop,
            backup,
            rebuild_recovery,
            false,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn open_bound_observed(
        root: Arc<JournalRoot>,
        payload: &Path,
        now: ScheduleNow,
        executor: Option<Arc<dyn GenerateExecutor + Send + Sync>>,
        external: Checkpoint,
        stop: Arc<AtomicBool>,
        backup: BackupWebDeps,
        rebuild_recovery: bool,
        observe_daily: bool,
    ) -> Result<Self, String> {
        if HOST_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("foreground processing host already active".into());
        }
        let result = (|| {
            root.revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            let journal = root.canonical_path().to_path_buf();
            let daily_observation = observe_daily.then(|| {
                Arc::new(crate::daily_queue_observation::DailyQueueTap::new(
                    Arc::clone(&root),
                ))
            });
            let failed = Arc::new(AtomicBool::new(false));
            let cancelled: Checkpoint = {
                let stop = Arc::clone(&stop);
                let failed = Arc::clone(&failed);
                let root = Arc::clone(&root);
                Arc::new(move || {
                    if failed.load(Ordering::Acquire)
                        || external()
                        || root.revalidate_canonical_binding().is_err()
                    {
                        stop.store(true, Ordering::Release);
                    }
                    stop.load(Ordering::Acquire)
                })
            };
            if cancelled() {
                return Err("thinking cancelled".into());
            }
            // Recovery is a startup action, before any callable use can be live.
            let cortex_owner = Arc::new(CallableCortex::open(&journal)?);
            let generator = HostGenerator {
                root: Arc::clone(&root),
                injected: executor,
            };
            let config = journal.join("config/schedules.json");
            let state = journal.join("health/scheduler.json");
            solstone_core_system::schedule::initialize_schedule_config(&config)
                .map_err(|e| e.to_string())?;
            let (mut engine, _) =
                ScheduleEngine::init(&config, &state, now).map_err(|e| e.to_string())?;
            let mut fresh = BTreeSet::new();
            match engine.register_defaults() {
                Ok(added) => fresh.extend(added),
                Err(error) => log::warn!("foreground schedule defaults: {error}"),
            }
            match solstone_core_maintenance::schedule_sync::sync(
                &config,
                solstone_core_maintenance::registry::routines(),
            ) {
                Ok(summary) => {
                    fresh.extend(
                        summary
                            .added
                            .iter()
                            .map(|id| solstone_core_maintenance::schedule_sync::schedule_name(id)),
                    );
                    match ScheduleEngine::init(&config, &state, now) {
                        Ok((reloaded, _)) => engine = reloaded,
                        Err(error) => log::warn!("foreground schedule reload: {error}"),
                    }
                }
                Err(error) => log::warn!("foreground schedule sync: {error}"),
            }
            let schedule_engine = Arc::new(Mutex::new(engine));
            let flush = Arc::new(Mutex::new(FlushState::default()));
            #[cfg(feature = "audio")]
            let audio_state = Arc::new(Mutex::new(None));
            let running = RunningUses::default();
            let payload_path = payload.to_path_buf();
            let hosted = Arc::new(HostedCommand {
                rebuild_recovery,
                journal: journal.clone(),
                payload: payload_path.clone(),
                root: Arc::clone(&root),
                talent_root: payload.join("solstone/talent"),
                apps_root: payload.join("solstone/apps"),
                templates_dir: payload.join("solstone/think/templates"),
                generator: generator.clone(),
                cancelled: Arc::clone(&cancelled),
                cortex_owner,
                running: running.clone(),
                failed: Arc::clone(&failed),
                backup,
                #[cfg(feature = "audio")]
                audio_state: Arc::clone(&audio_state),
            });
            let worker: CallableQueueWorker =
                Arc::new(move |args, stop| Self::execute_command(&hosted, args, stop));
            let admitted_cancelled = Arc::clone(&cancelled);
            let queue_slot = Arc::new(OnceLock::new());
            let queue = Arc::new(TaskQueue::with_callable_worker_and_admission(
                TaskQueueOptions {
                    #[cfg(windows)]
                    read_file_grants: Vec::new(),
                    journal_root: journal.clone(),
                    cap_resolver: Arc::new(DefaultCapResolver::default()),
                    process_state_probe: Arc::new(NoProcesses),
                    queue_sink: Some(Arc::new(HostQueueSink {
                        daily_observation: daily_observation.clone(),
                        queue: Arc::clone(&queue_slot),
                        schedule_engine: Arc::clone(&schedule_engine),
                        flush: Arc::clone(&flush),
                        cancelled: Arc::clone(&cancelled),
                    })),
                    process_sink: None,
                    ready: true,
                    before_deadline_commit: None,
                    child_environment: BTreeMap::new(),
                    task_binary: None,
                },
                Arc::clone(&worker),
                Arc::new(move |_args| !admitted_cancelled()),
                Some((
                    payload.join("solstone/talent"),
                    payload.join("solstone/apps"),
                )),
            ));
            if queue_slot.set(Arc::downgrade(&queue)).is_err() {
                log::warn!("foreground schedule queue was already installed");
            } else if let Ok(mut engine) = schedule_engine.lock() {
                engine.catch_up(
                    now,
                    &HostScheduleSink {
                        queue: queue.clone(),
                    },
                    &fresh,
                );
            }
            let discovery_checkpoint = Arc::clone(&cancelled);
            let discovery = WorkDiscovery::new_with_talent_roots_and_checkpoint(
                journal.clone(),
                queue.as_ref().clone(),
                true,
                solstone_core_think_cli::planner::DiscoveryTalentRoots {
                    rebuild_recovery,
                    talent: payload.join("solstone/talent"),
                    apps: payload.join("solstone/apps"),
                },
                Arc::new(move || {
                    if discovery_checkpoint() {
                        Err("thinking cancelled".into())
                    } else {
                        Ok(())
                    }
                }),
            );
            Ok(Self {
                daily_observation,
                journal,
                payload: payload_path,
                root,
                queue,
                schedule_engine,
                flush,
                discovery: Mutex::new(Some(discovery)),
                sense_handle: Mutex::new(None),
                schedule_handle: Mutex::new(None),
                running,
                stop,
                cancelled,
                failed,
                generator,
                #[cfg(test)]
                worker,
                #[cfg(feature = "audio")]
                audio_state,
            })
        })();
        if result.is_err() {
            HOST_ACTIVE.store(false, Ordering::Release);
        }
        result
    }
    fn execute_command(
        hosted: &HostedCommand,
        raw: &[String],
        control: CallableTaskContext,
    ) -> i32 {
        let checkpoint: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = {
            let host = Arc::clone(&hosted.cancelled);
            let control = control.clone();
            Arc::new(move || {
                if control.is_cancelled() || host() {
                    Err("cancelled".into())
                } else {
                    Ok(())
                }
            })
        };
        if control.is_cancelled() || (hosted.cancelled)() {
            return 1;
        }
        if !admit_command(&hosted.journal, raw) {
            return UNCOMPLETED_HOLD_EXIT;
        }
        let Some(args) = admit_argv(raw) else {
            return scheduled_commands::run(hosted, &strip_launcher(raw), &checkpoint);
        };
        let value = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let day = value("--day").unwrap_or_else(|| {
            Utc::now()
                .with_timezone(&solstone_core_journal_config::owner_zone(&hosted.journal))
                .format("%Y%m%d")
                .to_string()
        });
        let cancelled: Checkpoint = {
            let host = Arc::clone(&hosted.cancelled);
            let control = control.clone();
            Arc::new(move || control.is_cancelled() || host())
        };
        let cortex = Arc::new(HostCortexBoundary {
            journal: hosted.journal.clone(),
            talent_root: hosted.talent_root.clone(),
            apps_root: hosted.apps_root.clone(),
            templates_dir: hosted.templates_dir.clone(),
            generator: hosted
                .generator
                .executor(Arc::clone(&cancelled), control.priority),
            client: CortexRequestClient::new(&hosted.journal, CortexRequestPolicy::think()),
            cancelled: Arc::clone(&cancelled),
            owner: Arc::clone(&hosted.cortex_owner),
            running: hosted.running.clone(),
        });
        let executor = Arc::new(HostSenseExecutor {
            journal: hosted.journal.clone(),
            payload: hosted.payload.clone(),
            generator: hosted.generator.clone(),
            admitted_priority: Some(control.priority),
            cancelled: Arc::clone(&cancelled),
            failed: Arc::clone(&hosted.failed),
            #[cfg(feature = "audio")]
            audio_state: Arc::clone(&hosted.audio_state),
        });
        let context = ThinkContext::hosted(
            &hosted.journal,
            day.clone(),
            hosted.journal.join("chronicle").join(&day),
            now_ms(),
            hosted.talent_root.clone(),
            hosted.apps_root.clone(),
            cortex,
            Arc::new(HostIndexBoundary {
                cancelled: Arc::clone(&cancelled),
            }),
            Arc::new(now_ms),
            None,
            cancelled,
        )
        .with_sense_executor(executor)
        .with_rebuild_recovery(hosted.rebuild_recovery);
        let result = solstone_core_think_cli::run_hosted_command(
            &context,
            &args,
            &ChildLaunchContext::default(),
        )
        .exit_code;
        let options = solstone_core_indexer_store::todo::DrainOptions {
            force: false,
            budget: Duration::from_secs(600),
        };
        if let Err(error) = solstone_core_indexer_store::todo::drain_with_checkpoint(
            &hosted.journal,
            options,
            checkpoint.as_ref(),
        ) {
            log::warn!("foreground search update: {error}");
        }
        result
    }
    #[cfg(test)]
    pub fn execute_worker_command(&self, args: &[String]) -> i32 {
        (self.worker)(
            args,
            CallableTaskContext {
                cancelled: Arc::clone(&self.stop),
                priority: solstone_core_system::priority::classify(
                    args,
                    Utc::now()
                        .with_timezone(&solstone_core_journal_config::owner_zone(&self.journal))
                        .date_naive(),
                ),
                deadline: Instant::now() + Duration::from_secs(480),
            },
        )
    }
    /// One discovery wake, then the host queue. Joining stops callable workers,
    /// so it waits until a command submitted by this wake has finished.
    ///
    /// Analysis stays deferred until this pass publishes. The pass admits a
    /// daily retry and then reads that unit again under the execution claim;
    /// the command must not be between its own claims during that read.
    #[cfg(all(test, feature = "full-tests"))]
    pub fn deliver_discovery_once(&self) {
        let finished = self.queue.history().len();
        let promised = discovery_promise(&self.journal);
        hold_analysis(&self.journal, true);
        if let Some(discovery) = self.discovery.lock().unwrap().as_ref() {
            discovery.request();
        }
        let discovered_by = Instant::now() + Duration::from_secs(10);
        loop {
            if self.queue.history().len() > finished {
                break;
            }
            if discovery_promise(&self.journal).is_some_and(|promise| Some(promise) != promised) {
                break;
            }
            assert!(
                Instant::now() < discovered_by,
                "discovery did not publish its pass"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        hold_analysis(&self.journal, false);
        self.queue.resume_processing();
        if self
            .queue
            .any_reference(|reference| reference.starts_with("supervisor-"))
        {
            let finished_by = Instant::now() + Duration::from_secs(10);
            while self.queue.history().len() <= finished {
                assert!(
                    Instant::now() < finished_by,
                    "discovered command did not finish"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.queue.resume_processing();
        self.queue
            .join_callable_workers()
            .expect("callable workers");
    }
    fn executor(&self) -> Arc<HostSenseExecutor> {
        Arc::new(HostSenseExecutor {
            journal: self.journal.clone(),
            payload: self.payload.clone(),
            generator: self.generator.clone(),
            admitted_priority: None,
            cancelled: Arc::clone(&self.cancelled),
            failed: Arc::clone(&self.failed),
            #[cfg(feature = "audio")]
            audio_state: Arc::clone(&self.audio_state),
        })
    }
    pub fn start_workers(&mut self) -> Result<(), String> {
        if self.sense_handle.lock().unwrap().is_some()
            || self.schedule_handle.lock().unwrap().is_some()
        {
            return Err("foreground workers already active".into());
        }
        if let Some(discovery) = self.discovery.lock().unwrap().as_ref() {
            discovery.request();
        }
        let journal = self.journal.clone();
        let executor = self.executor();
        let cancelled = Arc::clone(&self.cancelled);
        let sense = std::thread::Builder::new()
            .name("journal-sense".into())
            .spawn(move || {
                while !cancelled() {
                    if let Err(error) = Self::sense_pass(&journal, executor.clone()) {
                        log::warn!("foreground Sense: {error}");
                    }
                    if !cancelled() {
                        std::thread::park_timeout(Duration::from_secs(2));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        *self.sense_handle.lock().unwrap() = Some(sense);
        let engine = Arc::clone(&self.schedule_engine);
        let queue = self.queue.clone();
        let journal = self.journal.clone();
        let cancelled = Arc::clone(&self.cancelled);
        let flush = Arc::clone(&self.flush);
        let running = self.running.clone();
        let failed = Arc::clone(&self.failed);
        let schedule = std::thread::Builder::new()
            .name("journal-schedule".into())
            .spawn(move || {
                let started = Instant::now();
                let mut last_settle_scan = Instant::now();
                let mut last_index_check = Instant::now() - Duration::from_secs(30);
                let mut last_status = Instant::now() - STATUS_INTERVAL;
                while !cancelled() {
                    let tick = Instant::now();
                    if last_status.elapsed() >= STATUS_INTERVAL
                        && !failed.load(Ordering::Acquire)
                        && !queue.callable_workers_failed()
                    {
                        last_status = tick;
                        publish_status(&journal, &queue, &engine, &running, started);
                    }
                    queue.resume_processing();
                    let today = Utc::now()
                        .with_timezone(&solstone_core_journal_config::owner_zone(&journal))
                        .format("%Y%m%d")
                        .to_string();
                    if let Ok(mut flush) = flush.lock()
                        && !cancelled()
                    {
                        let force = day_still_closing(&journal, &queue, SystemTime::now());
                        check_segment_flush(&journal, &queue, &mut flush, force.as_deref(), tick);
                    }
                    if !cancelled() {
                        check_activity_settle(
                            &journal,
                            &queue,
                            &mut last_settle_scan,
                            tick,
                            now_ms(),
                            &today,
                        );
                    }
                    if last_index_check.elapsed() >= Duration::from_secs(30) {
                        last_index_check = tick;
                        match solstone_core_indexer_store::todo::has_due(&journal, now_ms() as u64)
                        {
                            Ok(true) if !queue.contains_reference("index-todo") => {
                                let _ = queue.submit(ExecutionRequest::Bus(BusTaskRequest {
                                    cmd: TaskArgv::from_wire(vec![
                                        "journal".into(),
                                        "indexer".into(),
                                        "drain".into(),
                                    ])
                                    .expect("fixed command"),
                                    reference: "index-todo".into(),
                                    day: None,
                                    scheduler_name: None,
                                    queue_if_active_cmd_differs: false,
                                    priority: None,
                                    daily_catchup_provenance: None,
                                }));
                            }
                            Ok(_) => {}
                            Err(error) => log::warn!("foreground search work list: {error}"),
                        }
                    }
                    if let Ok(mut engine) = engine.lock() {
                        if cancelled() {
                            break;
                        }
                        let now = ScheduleNow::in_zone(
                            Utc::now(),
                            solstone_core_journal_config::owner_zone(&journal),
                        );
                        if let Err(error) = engine.check(
                            now,
                            &HostScheduleSink {
                                queue: queue.clone(),
                            },
                        ) {
                            log::warn!("foreground schedule: {error}");
                        }
                    }
                    if !cancelled() {
                        std::thread::park_timeout(Duration::from_secs(1));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        *self.schedule_handle.lock().unwrap() = Some(schedule);
        Ok(())
    }
    pub fn check_schedule(&self, now: ScheduleNow) -> Result<CheckReport, ScheduleError> {
        self.schedule_engine.lock().unwrap().check(
            now,
            &HostScheduleSink {
                queue: self.queue.clone(),
            },
        )
    }
    pub fn queue(&self) -> &TaskQueue {
        &self.queue
    }
    #[cfg(feature = "audio")]
    pub fn enable_audio(
        &self,
        models: PathBuf,
        model: PathBuf,
        scratch: PathBuf,
    ) -> Result<(), String> {
        if (self.cancelled)() {
            return Err("thinking cancelled".into());
        }
        let mut state = self.audio_state.lock().unwrap();
        if (self.cancelled)() {
            return Err("thinking cancelled".into());
        }
        if state.is_none() {
            *state = Some(AudioConfig {
                worker: Arc::new(crate::audio::AudioWorker::for_host(
                    Arc::clone(&self.root),
                    models,
                    model,
                    scratch,
                    Arc::clone(&self.cancelled),
                )),
            });
        }
        if let Some(handle) = self.sense_handle.lock().unwrap().as_ref() {
            handle.thread().unpark();
        }
        Ok(())
    }
    #[cfg(feature = "audio")]
    pub fn cancel_audio(&self) {
        if let Some(config) = self.audio_state.lock().unwrap().as_ref() {
            config.worker.cancel();
        }
    }
    #[cfg(feature = "audio")]
    pub fn resume_audio(&self) {
        if let Some(config) = self.audio_state.lock().unwrap().as_ref() {
            config.worker.resume();
        }
    }
    #[cfg(feature = "audio")]
    pub fn audio_status(&self) -> Value {
        self.audio_state
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.worker.status())
            .unwrap_or_else(|| serde_json::json!({"state":"disabled"}))
    }
    /// A positive observation is useful after signalling cancellation. Busy,
    /// absent and unobserved states are unknown, without delaying Stop.
    #[cfg(feature = "audio")]
    pub fn native_call_active(&self) -> Option<bool> {
        self.audio_state
            .try_lock()
            .ok()?
            .as_ref()?
            .worker
            .native_call_active()
    }
    pub fn run_sense_pass(&self) -> Result<(), String> {
        Self::sense_pass(&self.journal, self.executor())
    }
    fn sense_pass(journal: &Path, executor: Arc<HostSenseExecutor>) -> Result<(), String> {
        if executor.cancelled() {
            return Ok(());
        }
        let chronicle = journal.join("chronicle");
        if !chronicle.exists() {
            return Ok(());
        }
        let mut days = fs::read_dir(&chronicle)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        days.sort_by_key(|e| e.file_name());
        for day in days {
            if executor.cancelled() {
                break;
            }
            if !day.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let day = day.file_name().to_string_lossy().into_owned();
            let request = solstone_core_sense::batch::BatchRequest {
                day,
                jobs: 1,
                reprocess: None,
                segment: None,
                stream: None,
                dry_run: false,
                verbose: false,
                debug: false,
            };
            match solstone_core_sense::batch::run_batch_with_executor(
                journal,
                &request,
                &ChildLaunchContext::default(),
                None,
                executor.clone(),
                solstone_core_sense::dispatch::BatchMarkerPolicy::AdvanceStream,
            ) {
                Ok(()) | Err(solstone_core_sense::batch::BatchError::LeaseContended { .. }) => {}
                Err(solstone_core_sense::batch::BatchError::WorkerJoin) => {
                    executor.failed.store(true, Ordering::Release);
                    return Err("Sense worker did not stop cleanly".into());
                }
                Err(error) => log::warn!("foreground Sense day: {error}"),
            }
        }
        Ok(())
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire) || self.queue.callable_workers_failed()
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(tap) = &self.daily_observation {
            tap.close();
        }
        if let Some(discovery) = self.discovery.lock().unwrap().as_ref() {
            discovery.stop();
        }
        for slot in [&self.sense_handle, &self.schedule_handle] {
            if let Some(handle) = slot.lock().unwrap().as_ref() {
                handle.thread().unpark();
            }
        }
    }
    pub fn shutdown(&self) -> Result<(), String> {
        self.request_stop();
        let mut failed = false;
        if let Some(mut discovery) = self.discovery.lock().unwrap().take() {
            failed |= discovery.shutdown_checked().is_err();
        }
        let _ = self.queue.shutdown();
        failed |= self.queue.join_callable_workers().is_err();
        for slot in [&self.sense_handle, &self.schedule_handle] {
            if let Some(handle) = slot.lock().unwrap().take() {
                failed |= handle.join().is_err();
            }
        }
        if failed || self.failed() {
            return Err("foreground worker failed to join".into());
        }
        HOST_ACTIVE.store(false, Ordering::Release);
        Ok(())
    }
}

#[cfg(all(test, feature = "full-tests"))]
fn discovery_promise(journal: &Path) -> Option<i64> {
    solstone_core_system::work_inventory::read(journal)
        .ok()
        .flatten()
        .and_then(|report| report.next_pass_ms)
}

#[cfg(all(test, feature = "full-tests"))]
fn hold_analysis(journal: &Path, held: bool) {
    let path = journal.join("config/journal.json");
    let mut value: Value =
        serde_json::from_slice(&fs::read(&path).expect("journal config")).expect("journal config");
    let processing = value
        .as_object_mut()
        .expect("journal config object")
        .entry("processing")
        .or_insert_with(|| Value::Object(Map::new()));
    let processing = processing.as_object_mut().expect("processing object");
    processing.insert("history_confirmed".into(), Value::Bool(true));
    if held {
        processing.insert("mode".into(), Value::String("deferred".into()));
        processing.insert(
            "gate".into(),
            serde_json::json!({
                "time_window": {"enabled": true, "start": "00:00", "end": "00:00"},
                "display_powersave": {"enabled": false}
            }),
        );
    } else {
        processing.remove("mode");
        processing.remove("gate");
    }
    fs::write(&path, serde_json::to_vec(&value).expect("journal config")).expect("journal config");
}

impl Drop for ForegroundProcessingHost {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod detector_contract {
    #[test]
    fn no_detector_reports_no_detection() {
        assert_eq!(
            solstone_core_depict::Detector::detect(&super::NoDetector, b""),
            Ok(None)
        );
    }

    #[cfg(not(feature = "rfdetr-native"))]
    #[test]
    fn unlinked_detector_is_an_error_not_an_absent_run() {
        let detector = crate::rfdetr::RfdetrDetector::new(std::path::Path::new("."));
        assert!(solstone_core_depict::Detector::detect(&detector, b"").is_err());
    }
}

#[cfg(all(test, feature = "processing", feature = "full-tests"))]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use solstone_core_generate::GeneratedResponse;
    use solstone_core_system::work_retry::{self, Unit};

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn write_admitted(path: &Path, bytes: &[u8]) {
        use sha2::{Digest, Sha256};
        use solstone_core_callosum::{
            DeviceIngestEvent, DurableEvent, FileDescriptor, append_durable_event,
        };
        fs::write(path, bytes).unwrap();
        let segment = path.parent().unwrap();
        let stream = segment.parent().unwrap();
        let day = stream.parent().unwrap();
        let filename = path.file_name().unwrap().to_str().unwrap();
        append_durable_event(
            segment,
            &DurableEvent::DeviceIngest(DeviceIngestEvent {
                record_type: "device_ingest".into(),
                record_version: 1,
                outcome: "accepted".into(),
                protocol_version: 3,
                cid: "fixture".into(),
                source: "image".into(),
                day: day.file_name().unwrap().to_str().unwrap().into(),
                stream: stream.file_name().unwrap().to_str().unwrap().into(),
                segment: segment.file_name().unwrap().to_str().unwrap().into(),
                meta: Default::default(),
                extra: Default::default(),
                files: vec![FileDescriptor {
                    submitted: filename.into(),
                    written: filename.into(),
                    size: bytes.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    extra: Default::default(),
                }],
            }),
        )
        .unwrap();
    }

    fn write_image(path: &Path) {
        write_admitted(path, TINY_PNG);
    }

    struct StubGenerateExecutor;

    impl GenerateExecutor for StubGenerateExecutor {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            let text = if request.context == "observe.depict" {
                "foreground summary phrase".to_string()
            } else {
                serde_json::json!({
                    "density": "active",
                    "content_type": "writing",
                    "activity_summary": "foreground summary phrase",
                    "entities": [],
                    "facets": [],
                    "speculative_facet": null,
                    "meeting_detected": false,
                    "speakers": [],
                    "recommend": {
                        "screen_record": false,
                        "speaker_attribution": false
                    },
                    "emotional_register": "neutral",
                    "topics": [],
                    "quotes": []
                })
                .to_string()
            };
            Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
                id: None,
                text,
                model: "stub-model".to_string(),
                usage: serde_json::json!({}),
                finish_reason: "stop".to_string(),
                thinking: None,
                schema_validation: None,
                input_budget: None,
                request_budget: None,
                inference: None,
                hints_applied: Vec::new(),
            })))
        }
    }

    fn backup_deps(journal: &Path) -> BackupWebDeps {
        BackupWebDeps::new(journal.to_path_buf())
    }

    fn setup_test_journal() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let payload = temp.path().join("journal-payload");
        fs::create_dir_all(payload.join("solstone/talent")).unwrap();
        fs::create_dir_all(payload.join("solstone/apps")).unwrap();
        fs::create_dir_all(payload.join("solstone/think/templates")).unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            serde_json::json!({
                "identity": { "timezone": "UTC" },
                "providers": { "active": { "provider": "openai", "model": "fixture" } },
                "processing": { "history_confirmed": true }
            })
            .to_string(),
        )
        .unwrap();
        (temp, payload)
    }

    #[test]
    fn running_uses_report_what_the_cortex_boundary_is_running() {
        let running = RunningUses::default();
        let mut request = CortexRequest::new("prompt", "sense");
        request.config.insert("provider".into(), json!("google"));
        let guard = running.begin("use-1", &request);
        let status = running.status().unwrap();
        assert_eq!(status["running_uses"], 1);
        assert_eq!(status["uses"][0]["use_id"], "use-1");
        assert_eq!(status["uses"][0]["name"], "sense");
        assert_eq!(status["uses"][0]["provider"], "google");
        drop(guard);
        let status = running.status().unwrap();
        assert_eq!(status["running_uses"], 0);
        assert_eq!(status["uses"], json!([]));
    }

    #[test]
    fn the_runtime_publishes_its_status_while_it_runs_and_not_after_stop() {
        use solstone_core_callosum::{CallosumReceiveEvent, CallosumSocketConnection};
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let socket = temp.path().join("health/callosum.sock");
        fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let bus = runtime
            .block_on(solstone_core_callosum::CallosumSocketServer::bind(&socket))
            .unwrap();
        let mut observer = runtime.block_on(async {
            let mut observer = CallosumSocketConnection::new(&socket, Map::new());
            observer.start();
            observer
        });
        let mut host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();
        host.discovery
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .shutdown_checked()
            .unwrap();
        host.start_workers().unwrap();
        let (supervisor, cortex) = runtime.block_on(async {
            let mut supervisor = None;
            let mut cortex = None;
            let _ = tokio::time::timeout(Duration::from_secs(20), async {
                while supervisor.is_none() || cortex.is_none() {
                    match observer.next_event().await {
                        Some(CallosumReceiveEvent::Envelope { envelope, .. })
                            if envelope.event == "status" =>
                        {
                            match envelope.tract.as_str() {
                                "supervisor" => supervisor = Some(envelope.extra),
                                "cortex" => cortex = Some(envelope.extra),
                                _ => {}
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            })
            .await;
            (supervisor, cortex)
        });
        let supervisor = supervisor.expect("the runtime publishes its status");
        let services = supervisor["services"].as_array().unwrap();
        assert_eq!(
            services.len(),
            1,
            "the runtime is the one service: {services:?}"
        );
        assert_eq!(services[0]["name"], "supervisor");
        assert_eq!(services[0]["pid"], std::process::id());
        assert_eq!(supervisor["crashed"], json!([]));
        assert!(supervisor["queues"].is_object());
        assert!(supervisor["schedules"].is_array());
        let cortex = cortex.expect("the runtime publishes what it is thinking");
        assert_eq!(cortex["running_uses"], 0);
        assert_eq!(cortex["uses"], json!([]));

        host.shutdown().unwrap();
        let after_stop = runtime.block_on(async {
            tokio::time::timeout(STATUS_INTERVAL * 2, async {
                loop {
                    match observer.next_event().await {
                        Some(CallosumReceiveEvent::Envelope { envelope, .. })
                            if envelope.event == "status" =>
                        {
                            return true;
                        }
                        Some(_) => {}
                        None => return false,
                    }
                }
            })
            .await
            .unwrap_or(false)
        });
        assert!(!after_stop, "a stopped runtime publishes no status");
        runtime.block_on(observer.stop());
        runtime.block_on(bus.stop());
    }

    #[test]
    fn schedule_check_submits_cadence_and_records_outcome_without_writing_cadence_timestamp() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let start_time = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            start_time,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        let check_time = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 5, 0).unwrap(),
            chrono_tz::UTC,
        );
        let report = host.check_schedule(check_time).unwrap();
        assert!(
            report.submitted.contains(&"cadence".to_string()),
            "cadence entry should be submitted at 12:05"
        );
        wait_for_file(&temp.path().join("health/scheduler.json"));
        host.shutdown().unwrap();

        // A no-input twin writes no segment output and no health/cadence.json timestamp
        assert!(
            !temp.path().join("health/cadence.json").exists(),
            "health/cadence.json must not be written by host or no-input cadence"
        );

        // The scheduler state still records that command's outcome in health/scheduler.json
        assert!(
            temp.path().join("health/scheduler.json").exists(),
            "health/scheduler.json must exist"
        );
    }

    #[test]
    fn admitted_image_runs_depict_and_indexes_phrase_while_audio_phrase_is_absent() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        fs::write(
            payload.join("solstone/talent/sense.md"),
            "{\n\"type\": \"generate\",\n\"schedule\": \"segment\",\n\"priority\": 1,\n\"output\": \"json\",\n\"max_output_tokens\": 1024\n}\n\nAnalyze segment\n",
        )
        .unwrap();
        let segment_dir = temp.path().join("chronicle/20261012/notes/120000_1");
        fs::create_dir_all(&segment_dir).unwrap();

        let image_path = segment_dir.join("depict.png");
        write_image(&image_path);

        let audio_path = segment_dir.join("audio.jsonl");
        fs::write(
            &audio_path,
            b"{}\n{\"start\":\"00:00:01\",\"text\":\"exclusive audio only phrase transcript\"}\n",
        )
        .unwrap();

        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        host.run_sense_pass().unwrap();

        let depict_output = segment_dir.join("depict.jsonl");
        assert!(
            depict_output.exists(),
            "depict.jsonl must be written for admitted image"
        );
        let depict_content = fs::read_to_string(&depict_output).unwrap();
        assert!(
            depict_content.contains("foreground summary phrase"),
            "depict output must contain foreground summary phrase"
        );

        let think_res = host.execute_worker_command(&[
            "think".into(),
            "--day".into(),
            "20261012".into(),
            "--segment".into(),
            "120000_1".into(),
            "--stream".into(),
            "notes".into(),
        ]);
        assert_eq!(think_res, 0, "segment think command must succeed");

        let activity_path = segment_dir.join("talents/activity.md");
        assert!(
            activity_path.exists(),
            "talents/activity.md must exist after segment think"
        );
        let activity_content = fs::read_to_string(&activity_path).unwrap();
        assert!(
            activity_content.contains("foreground summary phrase"),
            "activity.md must contain foreground summary phrase"
        );

        solstone_core_indexer_store::scan::scan_journal(temp.path(), true).unwrap();

        let conn = solstone_core_indexer_store::open_index_reader(temp.path()).unwrap();
        let mut stmt = conn
            .prepare("SELECT content, path FROM chunks WHERE content MATCH ?")
            .unwrap();
        let rows = stmt
            .query_map(["\"foreground summary phrase\""], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(
            !rows.is_empty(),
            "index must contain an entry for foreground summary phrase"
        );
        assert!(
            rows.iter()
                .any(|(_content, path)| path.contains("120000_1")),
            "indexed row path must contain the source segment identity 120000_1"
        );

        // A phrase that exists only in an audio.jsonl transcript is absent from depict's chunks
        let mut audio_stmt = conn
            .prepare("SELECT content FROM chunks WHERE content MATCH ?")
            .unwrap();
        let audio_rows = audio_stmt
            .query_map(["\"exclusive audio only phrase transcript\""], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            audio_rows.is_empty(),
            "phrase that exists only in audio.jsonl must be absent from depict index"
        );
    }

    #[test]
    fn cancel_leaves_paths_unchanged_while_uncancelled_writes_them() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let segment_dir = temp.path().join("chronicle/20261012/notes/120000_2");
        fs::create_dir_all(&segment_dir).unwrap();

        let image_path = segment_dir.join("depict.png");
        write_image(&image_path);

        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        // 1. Cancel true leaves paths unchanged
        host.stop.store(true, Ordering::Release);
        host.run_sense_pass().unwrap();

        let depict_output = segment_dir.join("depict.jsonl");
        assert!(
            !depict_output.exists(),
            "depict.jsonl must not be written when cancelled"
        );

        // 2. Uncancelled twin writes them
        host.stop.store(false, Ordering::Release);
        host.run_sense_pass().unwrap();
        assert!(
            depict_output.exists(),
            "depict.jsonl must be written when uncancelled"
        );
    }

    #[test]
    fn missing_sense_json_exits_early_and_leaves_seeded_cadence_timestamp_unchanged() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let segment_dir = temp.path().join("chronicle/20261012/notes/120000_3");
        fs::create_dir_all(&segment_dir).unwrap();

        // Seed health/cadence.json with a timestamp
        fs::create_dir_all(temp.path().join("health")).unwrap();
        let cadence_path = temp.path().join("health/cadence.json");
        let seeded = serde_json::json!({
            "talents": {
                "schedule": 1700000000
            }
        });
        fs::write(&cadence_path, seeded.to_string()).unwrap();

        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        // Submit segment think command where sense.json does NOT exist
        let _ = host.execute_worker_command(&[
            "think".into(),
            "--day".into(),
            "20261012".into(),
            "--segment".into(),
            "120000_3".into(),
            "--stream".into(),
            "notes".into(),
        ]);

        // A later talent (e.g. schedule.jsonl) is not published
        assert!(
            !segment_dir.join("schedule.jsonl").exists(),
            "later talent schedule.jsonl must not be published when sense.json is missing"
        );

        // Seeded health/cadence.json timestamp stays put
        let current_cadence: Value =
            serde_json::from_str(&fs::read_to_string(&cadence_path).unwrap()).unwrap();
        assert_eq!(
            current_cadence["talents"]["schedule"], 1700000000,
            "seeded cadence timestamp must remain unchanged"
        );
    }

    #[test]
    fn unbound_retry_permission_cannot_publish_or_consume() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let segment = temp.path().join("chronicle/20261012/notes/120000_4");
        fs::create_dir_all(&segment).unwrap();
        write_image(&segment.join("depict.png"));
        let unit = Unit::Raw {
            day: "20261012".into(),
            stream: "notes".into(),
            segment: "120000_4".into(),
            handler: "depict".into(),
            output: "depict.jsonl".into(),
            direct: false,
        };
        work_retry::observe_held(temp.path(), &unit, "generate-wire-failed", "unbound-input")
            .unwrap();
        work_retry::request(temp.path(), &unit).unwrap();
        let id = unit.id().unwrap();
        let record = work_retry::read(temp.path(), &id).unwrap().unwrap();
        let permission = record.permission("unbound-input", &[]).unwrap();
        let ledger = temp
            .path()
            .join("health/work-retry")
            .join(format!("{id}.json"));
        let before = fs::read(&ledger).unwrap();
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            ScheduleNow::in_zone(
                Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
                chrono_tz::UTC,
            ),
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();
        assert_ne!(
            host.execute_worker_command(&[
                "think".into(),
                "--day".into(),
                "20261012".into(),
                "--retry-unit".into(),
                id,
                "--retry-cause".into(),
                permission.cause,
            ]),
            0
        );
        assert!(!segment.join("depict.jsonl").exists());
        assert_eq!(fs::read(ledger).unwrap(), before);
    }

    #[test]
    fn describe_and_daily_only_think_stay_held() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let segment_dir = temp.path().join("chronicle/20261012/notes/120000_5");
        fs::create_dir_all(&segment_dir).unwrap();

        let unit = Unit::Raw {
            day: "20261012".into(),
            stream: "notes".into(),
            segment: "120000_5".into(),
            handler: "describe".into(),
            output: "describe.jsonl".into(),
            direct: false,
        };
        let id = unit.id().unwrap();
        work_retry::observe_held(temp.path(), &unit, "generate-wire-failed", "sample.png").unwrap();

        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        // Submit describe command - should be held (exit code non-zero)
        let describe_res = host.execute_worker_command(&["describe".into(), "sample.png".into()]);
        assert_ne!(
            describe_res, 0,
            "describe command must exit with non-zero code"
        );

        // Submit daily-only think command - should be held (exit code non-zero)
        let daily_res =
            host.execute_worker_command(&["think".into(), "--day".into(), "20261012".into()]);
        assert_ne!(
            daily_res, 0,
            "daily-only think must exit with non-zero code"
        );

        // The retry record is still held
        let record = work_retry::read(temp.path(), &id).unwrap().unwrap();
        assert!(record.held, "retry record must remain held");
    }

    #[test]
    fn retry_raw_unit_depict_executes_in_foreground_host_and_clears_permission() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let segment_dir = temp.path().join("chronicle/20261012/notes/120000_6");
        fs::create_dir_all(&segment_dir).unwrap();
        let image_path = segment_dir.join("depict.png");
        write_image(&image_path);

        let unit = Unit::Raw {
            day: "20261012".into(),
            stream: "notes".into(),
            segment: "120000_6".into(),
            handler: "depict".into(),
            output: "depict.jsonl".into(),
            direct: false,
        };
        let id = unit.id().unwrap();

        let depict_output = segment_dir.join("depict.jsonl");
        let header = serde_json::json!({
            "raw": "depict.png",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "failed",
                "handler": "depict",
                "reason_code": "generate-wire-failed",
                "attempts": 3,
            }
        });
        fs::write(&depict_output, format!("{header}\n")).unwrap();

        let evidence =
            solstone_core_system::work_retry::raw::inspect_output(temp.path(), &depict_output)
                .unwrap()
                .unwrap();
        solstone_core_system::work_retry::raw::observe(temp.path(), &evidence).unwrap();
        let record = work_retry::read(temp.path(), &id).unwrap().unwrap();
        assert!(record.held, "retry record must be held");

        work_retry::request(temp.path(), &unit).unwrap();
        let perm = work_retry::read(temp.path(), &id)
            .unwrap()
            .unwrap()
            .permission(&evidence.input, &[])
            .expect("retry permission must be granted");

        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        let exit_code = host.execute_worker_command(&[
            "think".into(),
            "--day".into(),
            "20261012".into(),
            "--retry-unit".into(),
            id.clone(),
            "--retry-cause".into(),
            perm.cause.clone(),
        ]);
        assert_eq!(exit_code, 0, "worker retry depict command must exit with 0");

        assert!(depict_output.exists(), "depict.jsonl must exist");
        let content = fs::read_to_string(&depict_output).unwrap();
        assert!(
            content.contains("foreground summary phrase"),
            "depict.jsonl must contain foreground summary phrase"
        );

        let post_record = work_retry::read(temp.path(), &id).unwrap().unwrap();
        assert!(
            !post_record.held,
            "retry record must not be held after finish"
        );
        assert!(
            post_record.permission(&evidence.input, &[]).is_none(),
            "permission must be gone after finish"
        );

        // Run identical command a second time
        let second_exit = host.execute_worker_command(&[
            "think".into(),
            "--day".into(),
            "20261012".into(),
            "--retry-unit".into(),
            id.clone(),
            "--retry-cause".into(),
            perm.cause.clone(),
        ]);
        assert_ne!(
            second_exit, 0,
            "second retry attempt must not finish successfully"
        );
        let second_content = fs::read_to_string(&depict_output).unwrap();
        assert_eq!(
            content, second_content,
            "output bytes must not be rewritten on second run"
        );
    }

    #[test]
    fn retry_raw_unit_describe_stays_held_in_foreground_host() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let segment_dir = temp.path().join("chronicle/20261012/notes/120000_7");
        fs::create_dir_all(&segment_dir).unwrap();
        let image_path = segment_dir.join("sample.png");
        write_image(&image_path);

        let unit = Unit::Raw {
            day: "20261012".into(),
            stream: "notes".into(),
            segment: "120000_7".into(),
            handler: "describe".into(),
            output: "describe.jsonl".into(),
            direct: false,
        };
        let id = unit.id().unwrap();

        work_retry::observe_held(temp.path(), &unit, "generate-wire-failed", "sample.png").unwrap();
        let record = work_retry::read(temp.path(), &id).unwrap().unwrap();
        assert!(record.held, "retry record must be held");

        work_retry::request(temp.path(), &unit).unwrap();
        let perm = work_retry::read(temp.path(), &id)
            .unwrap()
            .unwrap()
            .permission("sample.png", &[])
            .expect("retry permission must be granted");

        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();

        let exit_code = host.execute_worker_command(&[
            "think".into(),
            "--day".into(),
            "20261012".into(),
            "--retry-unit".into(),
            id.clone(),
            "--retry-cause".into(),
            perm.cause.clone(),
        ]);
        assert_ne!(exit_code, 0, "describe retry command must exit non-zero");

        let describe_output = segment_dir.join("describe.jsonl");
        assert!(!describe_output.exists(), "describe.jsonl must not exist");

        let post_record = work_retry::read(temp.path(), &id).unwrap().unwrap();
        assert!(post_record.held, "retry record must remain held");
    }

    #[test]
    fn missing_daily_retry_permission_stays_refused() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let now = ScheduleNow::in_zone(
            Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
            chrono_tz::UTC,
        );
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            now,
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();
        let id = "0000000000000000000000000000000000000000000000000000000000000000";
        let exit_code = host.execute_worker_command(&[
            "think".into(),
            "--day".into(),
            "20261012".into(),
            "--retry-unit".into(),
            id.into(),
            "--retry-cause".into(),
            "owner:unused".into(),
        ]);
        assert_ne!(exit_code, 0);
        assert!(work_retry::read(temp.path(), id).unwrap().is_none());
        let health_dir = temp.path().join("chronicle/20261012/health");
        let oplogs = if health_dir.exists() {
            fs::read_dir(&health_dir)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    name.contains("oplog") || name.contains("think")
                })
                .count()
        } else {
            0
        };
        assert_eq!(oplogs, 0);
    }
    fn wait_for_file(path: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while !path.is_file() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            path.is_file(),
            "automatic publication missing: {}",
            path.display()
        );
    }

    #[test]
    fn sense_worker_processes_new_input_after_its_initial_batch() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let first = temp
            .path()
            .join("chronicle/20261012/notes/120000_1/depict.png");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        write_image(&first);
        let mut host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            Arc::new(StubGenerateExecutor),
            backup_deps(temp.path()),
        )
        .unwrap();
        // Isolate the Sense loop: discovery must not supply a second execution.
        host.discovery
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .shutdown_checked()
            .unwrap();
        host.start_workers().unwrap();
        wait_for_file(&first.with_extension("jsonl"));
        let second = temp
            .path()
            .join("chronicle/20261013/notes/120000_2/depict.png");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        write_image(&second);
        wait_for_file(&second.with_extension("jsonl"));
        host.shutdown().unwrap();
        assert!(host.sense_handle.lock().unwrap().is_none());
        assert!(host.schedule_handle.lock().unwrap().is_none());
    }

    #[test]
    fn image_waits_for_accepted_identical_custody_before_generate() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let image = temp
            .path()
            .join("chronicle/20261012/notes/120000_1/depict.png");
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        fs::write(&image, TINY_PNG).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let executor = move |request: &GenerateRequest| {
            count.fetch_add(1, Ordering::SeqCst);
            StubGenerateExecutor.execute(request)
        };
        let host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            Arc::new(executor),
            backup_deps(temp.path()),
        )
        .unwrap();
        host.run_sense_pass().unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "pre-receipt bytes cannot enter Generate"
        );
        assert!(!image.with_extension("jsonl").exists());
        write_image(&image);
        let mut changed = TINY_PNG.to_vec();
        changed[30] ^= 1;
        fs::write(&image, changed).unwrap();
        host.run_sense_pass().unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a different hash cannot borrow the accepted receipt"
        );
        assert!(!image.with_extension("jsonl").exists());
        fs::write(&image, TINY_PNG).unwrap();
        host.run_sense_pass().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(image.with_extension("jsonl").is_file());
        host.shutdown().unwrap();
    }

    #[test]
    fn stop_signals_held_generate_and_keeps_root_until_actual_worker_join() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (temp, payload) = setup_test_journal();
        let image = temp
            .path()
            .join("chronicle/20261012/notes/120000_1/depict.png");
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        write_image(&image);
        let (entered_send, entered) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(released);
        let executor = move |request: &GenerateRequest| {
            entered_send.send(()).unwrap();
            released.lock().unwrap().recv().unwrap();
            StubGenerateExecutor.execute(request)
        };
        let mut host = ForegroundProcessingHost::open_with_executor(
            temp.path(),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            Arc::new(executor),
            backup_deps(temp.path()),
        )
        .unwrap();
        host.discovery
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .shutdown_checked()
            .unwrap();
        let root = Arc::downgrade(&host.root);
        let stop = Arc::clone(&host.stop);
        host.start_workers().unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        // This is the native early Stop token, signalled without the host mutex.
        stop.store(true, Ordering::Release);
        let (drained_send, drained) = std::sync::mpsc::channel();
        let retirement = std::thread::spawn(move || {
            let result = host.shutdown();
            drained_send.send(result).unwrap();
        });
        assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(root.upgrade().is_some());
        assert!(!image.with_extension("jsonl").exists());
        release.send(()).unwrap();
        drained
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        retirement.join().unwrap();
        assert!(!image.with_extension("jsonl").exists());
        assert!(
            root.upgrade().is_none(),
            "original root may drop only after actual drain"
        );
    }

    const OCR_GAP_DETAIL: &str = crate::screen_ocr::PINNED_MODELS_DETAIL;

    fn corpus(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../core/fixtures/describe_corpus")
            .join(name)
    }

    fn probe_mp4() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "../../core/crates/solstone-core-describe/tests/fixtures/delayed_video_probe_screen.mp4",
        )
    }

    fn segment_dir(journal: &Path, name: &str) -> PathBuf {
        let path = journal.join("chronicle/20261012/screen").join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn admit_copy(segment: &Path, source: &Path) -> PathBuf {
        let bytes = fs::read(source).unwrap();
        let path = segment.join(source.file_name().unwrap());
        write_admitted(&path, &bytes);
        path
    }

    fn context_for(path: &Path) -> solstone_core_sense::work::SegmentContext {
        let segment = path.parent().unwrap();
        let stream = segment.parent().unwrap();
        let day = stream.parent().unwrap();
        solstone_core_sense::work::SegmentContext {
            key: solstone_core_sense::work::SegmentKey {
                day: day.file_name().unwrap().to_str().unwrap().into(),
                stream: Some(stream.file_name().unwrap().to_str().unwrap().into()),
                segment: segment.file_name().unwrap().to_str().unwrap().into(),
            },
            cid: Some("fixture".into()),
            source: Some("screen".into()),
            batch: false,
            meta: None,
        }
    }

    fn open_stub(journal: &Path, payload: &Path) -> ForegroundProcessingHost {
        open_executor(journal, payload, Arc::new(StubGenerateExecutor))
    }

    fn open_executor(
        journal: &Path,
        payload: &Path,
        executor: Arc<dyn GenerateExecutor + Send + Sync>,
    ) -> ForegroundProcessingHost {
        ForegroundProcessingHost::open_with_executor(
            journal,
            payload,
            ScheduleNow::in_zone(
                Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap(),
                chrono_tz::UTC,
            ),
            executor,
            backup_deps(journal),
        )
        .unwrap()
    }

    fn run_handler(host: &ForegroundProcessingHost, handler: &str, input: &Path) -> SenseJobResult {
        host.executor().execute(
            handler,
            input,
            &context_for(input),
            None,
            &AtomicBool::new(false),
        )
    }

    fn reset_describe_probes() -> (
        std::sync::MutexGuard<'static, ()>,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let host = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let guard = super::DESCRIBE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        super::DESCRIBE_CANCEL_AT.store(0, Ordering::SeqCst);
        super::DESCRIBE_POLLS.store(0, Ordering::SeqCst);
        *super::DESCRIBE_AFTER_ENTRY
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        *super::TEST_DEPICT_DETECTION
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        (guard, host)
    }

    fn jsonl_rows(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_distinct_frames(path: &Path, at_least: usize) {
        let rows = jsonl_rows(path);
        assert!(
            rows[0]
                .get("_solstone_processing")
                .and_then(Value::as_object)
                .is_some(),
            "processing header missing: {rows:?}"
        );
        let mut seen = BTreeSet::new();
        for row in rows.iter().skip(1) {
            let id = row["frame_id"].as_u64().expect("frame_id");
            let timestamp = row["timestamp"].as_f64().expect("timestamp");
            seen.insert((id, timestamp.to_bits()));
        }
        assert!(seen.len() >= at_least, "frames: {seen:?}");
    }

    fn assert_no_screen_text(segment: &Path) {
        assert!(!segment.join("talents/screen_text.md").exists());
    }

    fn assert_no_index_hint(journal: &Path) {
        let status = solstone_core_indexer_store::todo::inspect(journal).unwrap();
        assert!(
            status.keys.is_empty(),
            "describe must not enqueue a success screen text: {:?}",
            status.keys
        );
    }

    fn ocr_sidecar(segment: &Path) -> Value {
        let mut paths = fs::read_dir(segment)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                name.starts_with(".screen-ocr-v3-")
                    && name.ends_with(".json")
                    && !name.contains("folded")
            })
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1, "{paths:?}");
        serde_json::from_slice(&fs::read(paths.remove(0)).unwrap()).unwrap()
    }

    fn generated_text(text: String) -> GenerateResponse {
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text,
            model: "stub-model".to_string(),
            usage: json!({}),
            finish_reason: "stop".to_string(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    struct MediaFrameExecutor;
    impl GenerateExecutor for MediaFrameExecutor {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            if request.context == "observe.describe.frame" {
                return Ok(generated_text(
                    json!({
                        "primary": "media",
                        "secondary": "none",
                        "overlap": false,
                        "visual_description": "kept screen description",
                        "density": "active",
                        "content_type": "writing",
                        "activity_summary": "foreground summary phrase",
                        "entities": [],
                        "facets": [],
                        "speculative_facet": null,
                        "meeting_detected": false,
                        "speakers": [],
                        "recommend": {"screen_record": false, "speaker_attribution": false},
                        "emotional_register": "neutral",
                        "topics": [],
                        "quotes": []
                    })
                    .to_string(),
                ));
            }
            GenerateExecutor::execute(&StubGenerateExecutor, request)
        }
    }

    struct BlockingProvider;
    impl GenerateExecutor for BlockingProvider {
        fn execute(&self, _request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            use solstone_core_generate::{RefusalReason, RefusedResponse};
            Ok(GenerateResponse::Refused(RefusedResponse {
                id: None,
                reason: RefusalReason::NoEngineConfigured,
                reason_code: None,
                retryable: false,
                blocking: true,
                reset_at_ms: None,
                provider: Some("fixture".into()),
                detail: "provider refused".into(),
            }))
        }
    }

    struct CancelAfterFirstGenerate;
    impl GenerateExecutor for CancelAfterFirstGenerate {
        fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
            let polls = super::DESCRIBE_POLLS.load(Ordering::SeqCst);
            super::DESCRIBE_CANCEL_AT.store(polls.saturating_add(1), Ordering::SeqCst);
            GenerateExecutor::execute(&StubGenerateExecutor, request)
        }
    }

    fn processing_header(state: &str, reason: &str) -> String {
        format!(
            "{}\n{{\"timestamp\":0.0,\"frame_id\":1}}\n",
            json!({
                "_solstone_processing": {
                    "schema": 1,
                    "state": state,
                    "reason_code": reason,
                    "handler": "describe",
                    "attempts": 1
                }
            })
        )
    }

    #[test]
    fn describe_history_requires_permission_and_import_is_admitted() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["processing"]["history_confirmed"] = json!(false);
        fs::write(&config_path, config.to_string()).unwrap();
        let policy = solstone_core_system::work_policy::WorkPolicy::read(temp.path()).unwrap();
        assert_eq!(
            policy.lane,
            solstone_core_system::work_policy::ThinkingLane::OwnKeys
        );
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let host = open_executor(
            temp.path(),
            &payload,
            Arc::new(move |request: &GenerateRequest| {
                count.fetch_add(1, Ordering::SeqCst);
                StubGenerateExecutor.execute(request)
            }),
        );
        let segment = segment_dir(temp.path(), "history-policy");
        let video = admit_copy(&segment, &probe_mp4());
        assert_eq!(run_handler(&host, "describe", &video), SenseJobResult::Hold);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!video.with_extension("jsonl").exists());

        let mut executor = host.executor();
        Arc::get_mut(&mut executor).unwrap().admitted_priority = Some(WorkPriority {
            band: WorkBand::Import,
            source_time: 0,
        });
        assert_eq!(
            executor.execute(
                "describe",
                &video,
                &context_for(&video),
                None,
                &AtomicBool::new(false)
            ),
            SenseJobResult::Finished(0)
        );
        assert!(calls.load(Ordering::SeqCst) > 0);

        let confirmed_segment = segment_dir(temp.path(), "history-confirmed");
        let confirmed = admit_copy(&confirmed_segment, &probe_mp4());
        config["processing"]["history_confirmed"] = json!(true);
        fs::write(&config_path, config.to_string()).unwrap();
        assert_eq!(
            run_handler(&host, "describe", &confirmed),
            SenseJobResult::Finished(0)
        );
    }

    #[test]
    fn describe_rechecks_spending_permission_before_each_request() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let config_path = temp.path().join("config/journal.json");
        let path = config_path.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let host = open_executor(
            temp.path(),
            &payload,
            Arc::new(move |request: &GenerateRequest| {
                count.fetch_add(1, Ordering::SeqCst);
                let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                config["processing"]["history_confirmed"] = json!(false);
                fs::write(&path, config.to_string()).unwrap();
                StubGenerateExecutor.execute(request)
            }),
        );
        let segment = segment_dir(temp.path(), "revoke-history");
        let video = admit_copy(&segment, &corpus("mixed_vp8_screen.webm"));
        assert_ne!(
            run_handler(&host, "describe", &video),
            SenseJobResult::Finished(0)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!video.with_extension("jsonl").exists());
        assert_no_screen_text(&segment);
        assert_no_index_hint(temp.path());
    }

    #[test]
    fn accepted_screen_videos_publish_rows_and_the_ocr_gap() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let host = open_stub(temp.path(), &payload);
        for (source, at_least) in [
            (corpus("mixed_vp8_screen.webm"), 2),
            (corpus("mixed_h264_screen.mov"), 2),
            (probe_mp4(), 1),
        ] {
            let segment = segment_dir(temp.path(), source.file_stem().unwrap().to_str().unwrap());
            let video = admit_copy(&segment, &source);
            let original = fs::read(&video).unwrap();
            assert_eq!(
                run_handler(&host, "describe", &video),
                SenseJobResult::Finished(0)
            );
            assert_eq!(fs::read(&video).unwrap(), original);
            let output = video.with_extension("jsonl");
            assert_distinct_frames(&output, at_least);
            assert!(solstone_core_sense::unfinished_output(&video, "describe").is_none());
            let again = fs::read(&output).unwrap();
            assert_eq!(
                run_handler(&host, "describe", &video),
                SenseJobResult::Finished(0)
            );
            assert_eq!(fs::read(&output).unwrap(), again);
            let sidecar = ocr_sidecar(&segment);
            assert_eq!(sidecar["detail"], OCR_GAP_DETAIL);
            assert!(
                sidecar["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|frame| frame["error_code"] == "helper_unavailable")
            );
            assert_no_screen_text(&segment);
            assert_no_index_hint(temp.path());
        }
    }

    #[test]
    fn unfinished_output_keeps_failed_describe_unfinished() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let host = open_stub(temp.path(), &payload);
        for (name, state, reason, finished) in [
            ("analyzed", "analyzed", "ok", true),
            ("empty", "empty", "no_decodable_frames", true),
            ("failed", "failed", "corrupt_input", false),
        ] {
            let segment = segment_dir(temp.path(), name);
            let video = segment.join(format!("{name}.webm"));
            write_admitted(&video, b"eligibility");
            let output = video.with_extension("jsonl");
            let header = processing_header(state, reason);
            fs::write(&output, &header).unwrap();
            assert_eq!(
                solstone_core_sense::unfinished_output(&video, "describe").is_none(),
                finished,
                "{name}"
            );
            let before = fs::read(&output).unwrap();
            let result = run_handler(&host, "describe", &video);
            assert_eq!(
                result,
                if finished {
                    SenseJobResult::Finished(0)
                } else {
                    SenseJobResult::Finished(1)
                },
                "{name}"
            );
            assert_eq!(fs::read(&output).unwrap(), before, "{name}");
        }
    }

    #[test]
    fn malformed_decode_finishes_without_a_success_header() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let host = open_stub(temp.path(), &payload);
        let segment = segment_dir(temp.path(), "malformed");
        let video = admit_copy(&segment, &corpus("not_a_video_screen.webm"));
        let original = fs::read(&video).unwrap();
        assert_eq!(
            run_handler(&host, "describe", &video),
            SenseJobResult::Finished(1)
        );
        assert_eq!(fs::read(&video).unwrap(), original);
        let output = video.with_extension("jsonl");
        assert!(output.is_file());
        assert_eq!(
            jsonl_rows(&output)[0]["_solstone_processing"]["state"],
            "failed"
        );
        assert!(solstone_core_sense::unfinished_output(&video, "describe").is_some());
    }

    #[test]
    fn provider_refusal_does_not_promote() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let host = open_executor(temp.path(), &payload, Arc::new(BlockingProvider));
        let segment = segment_dir(temp.path(), "refused");
        let video = admit_copy(&segment, &probe_mp4());
        assert_eq!(
            run_handler(&host, "describe", &video),
            SenseJobResult::Finished(1)
        );
        assert!(!video.with_extension("jsonl").exists());
        assert_no_screen_text(&segment);
        assert_no_index_hint(temp.path());
    }

    #[test]
    fn detector_refusal_keeps_the_description() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let host = open_executor(temp.path(), &payload, Arc::new(MediaFrameExecutor));
        let segment = segment_dir(temp.path(), "detect");
        let video = admit_copy(&segment, &probe_mp4());
        assert_eq!(
            run_handler(&host, "describe", &video),
            SenseJobResult::Finished(0)
        );
        let rows = jsonl_rows(&video.with_extension("jsonl"));
        let row = &rows[1];
        assert_eq!(
            row["analysis"]["visual_description"],
            "kept screen description"
        );
        assert_eq!(row["detection_error"]["reason_code"], "rfdetr-unavailable");
        assert!(row.get("detections").is_none());
    }

    #[test]
    fn cancel_during_decode_and_between_units_holds() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let segment = segment_dir(temp.path(), "decode-cancel");
        let video = admit_copy(&segment, &corpus("mixed_vp8_screen.webm"));
        super::DESCRIBE_CANCEL_AT.store(2, Ordering::SeqCst);
        let host = open_stub(temp.path(), &payload);
        assert_eq!(run_handler(&host, "describe", &video), SenseJobResult::Hold);
        assert!(!video.with_extension("jsonl").exists());
        assert_no_screen_text(&segment);
        assert_no_index_hint(temp.path());

        drop(host);
        super::DESCRIBE_CANCEL_AT.store(0, Ordering::SeqCst);
        super::DESCRIBE_POLLS.store(0, Ordering::SeqCst);
        let between = segment_dir(temp.path(), "between-units");
        let second = admit_copy(&between, &corpus("mixed_h264_screen.mov"));
        let host = open_executor(temp.path(), &payload, Arc::new(CancelAfterFirstGenerate));
        assert_eq!(
            run_handler(&host, "describe", &second),
            SenseJobResult::Hold
        );
        assert!(
            super::DESCRIBE_CANCEL_AT.load(Ordering::SeqCst) > 0,
            "the second case must enter Generate before cancellation"
        );
        assert!(!second.with_extension("jsonl").exists());
        assert_no_index_hint(temp.path());
    }

    #[test]
    fn replacement_after_entry_leaves_the_prior_output_and_the_twin_completes() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let prior = segment_dir(temp.path(), "replaced");
        let video = admit_copy(&prior, &corpus("mixed_vp8_screen.webm"));
        let output = video.with_extension("jsonl");
        let header = processing_header("failed", "analysis_failed");
        fs::write(&output, &header).unwrap();
        let video_path = video.clone();
        *super::DESCRIBE_AFTER_ENTRY
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Box::new(move || {
            fs::write(&video_path, b"replaced-after-entry").unwrap();
        }));
        let host = open_stub(temp.path(), &payload);
        assert_eq!(run_handler(&host, "describe", &video), SenseJobResult::Hold);
        assert_eq!(fs::read_to_string(&output).unwrap(), header);
        assert_no_screen_text(&prior);
        assert_no_index_hint(temp.path());

        super::DESCRIBE_POLLS.store(0, Ordering::SeqCst);
        super::DESCRIBE_CANCEL_AT.store(0, Ordering::SeqCst);
        let twin_dir = segment_dir(temp.path(), "twin");
        let twin = admit_copy(&twin_dir, &corpus("mixed_vp8_screen.webm"));
        let original = fs::read(&twin).unwrap();
        assert_eq!(
            run_handler(&host, "describe", &twin),
            SenseJobResult::Finished(0)
        );
        assert_eq!(fs::read(&twin).unwrap(), original);
        assert_distinct_frames(&twin.with_extension("jsonl"), 2);
    }

    #[test]
    fn depict_still_records_detection_error_without_the_native_library() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let host = open_stub(temp.path(), &payload);
        let segment = segment_dir(temp.path(), "120001_1");
        let image = segment.join("depict.png");
        write_image(&image);
        assert_eq!(
            run_handler(&host, "depict", &image),
            SenseJobResult::Finished(0)
        );
        let rows = jsonl_rows(&image.with_extension("jsonl"));
        assert!(
            rows[1]["text"]
                .as_str()
                .unwrap()
                .contains("foreground summary phrase")
        );
        assert_eq!(
            rows[1]["detection_error"]["reason_code"],
            "rfdetr-unavailable"
        );
        assert!(rows[1].get("detections").is_none());
    }

    #[test]
    fn depict_records_detections_only_when_the_adapter_returns_some() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        *super::TEST_DEPICT_DETECTION
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(json!({
            "image": {"width": 1, "height": 1},
            "detections": [{"class_id": 85, "score": 0.8}]
        }));
        let host = open_stub(temp.path(), &payload);
        let segment = segment_dir(temp.path(), "120002_1");
        let image = segment.join("depict.png");
        write_image(&image);
        assert_eq!(
            run_handler(&host, "depict", &image),
            SenseJobResult::Finished(0)
        );
        let rows = jsonl_rows(&image.with_extension("jsonl"));
        assert!(
            rows[1]["text"]
                .as_str()
                .unwrap()
                .contains("foreground summary phrase")
        );
        assert!(rows[1].get("detection_error").is_none());
        assert_eq!(rows[1]["detections"]["objects"][0]["class_id"], 85);
    }

    #[cfg(feature = "rfdetr-native")]
    #[test]
    fn native_clock_png_detects_class_85() {
        let path = std::env::var("SOLSTONE_RFDETR_CLOCK_PNG").unwrap();
        let png = fs::read(path).unwrap();
        let value = solstone_core_depict::Detector::detect(
            &crate::rfdetr::RfdetrDetector::new(Path::new(".")),
            &png,
        )
        .unwrap()
        .expect("a real detection is Some");
        let detections = value["detections"].as_array().expect("detections");
        assert!(
            detections.iter().any(|detection| {
                detection["class_id"].as_i64() == Some(85)
                    && detection["score"].as_f64().is_some_and(|score| score > 0.5)
            }),
            "{value}"
        );
    }

    #[cfg(feature = "rfdetr-native")]
    #[test]
    fn native_blank_png_is_an_empty_detection_list() {
        let path = std::env::var("SOLSTONE_RFDETR_BLANK_PNG").unwrap();
        let png = fs::read(path).unwrap();
        let value = solstone_core_depict::Detector::detect(
            &crate::rfdetr::RfdetrDetector::new(Path::new(".")),
            &png,
        )
        .unwrap()
        .expect("an empty detection is Some, not None");
        assert!(value["detections"].as_array().unwrap().is_empty());
        assert!(value.get("detection_error").is_none());
    }

    #[cfg(feature = "rfdetr-native")]
    #[test]
    fn native_replacement_after_entry_leaves_the_prior_output() {
        let _probes = reset_describe_probes();
        let (temp, payload) = setup_test_journal();
        let prior = segment_dir(temp.path(), "native-replaced");
        let video = admit_copy(&prior, &corpus("mixed_vp8_screen.webm"));
        let output = video.with_extension("jsonl");
        let header = processing_header("failed", "analysis_failed");
        fs::write(&output, &header).unwrap();
        let video_path = video.clone();
        *super::DESCRIBE_AFTER_ENTRY
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Box::new(move || {
            fs::write(&video_path, b"replaced-after-entry").unwrap();
        }));
        let host = open_stub(temp.path(), &payload);
        assert_eq!(run_handler(&host, "describe", &video), SenseJobResult::Hold);
        assert_eq!(fs::read_to_string(&output).unwrap(), header);
        let twin_dir = segment_dir(temp.path(), "native-twin");
        super::DESCRIBE_POLLS.store(0, Ordering::SeqCst);
        let twin = admit_copy(&twin_dir, &corpus("mixed_h264_screen.mov"));
        assert_eq!(
            run_handler(&host, "describe", &twin),
            SenseJobResult::Finished(0)
        );
        assert_distinct_frames(&twin.with_extension("jsonl"), 2);
    }
}

#[cfg(all(test, feature = "processing", feature = "full-tests"))]
mod processing_proofs {
    use super::*;
    use chrono::{Datelike, TimeZone};
    use serde_json::json;
    use solstone_core_backup::Destination;
    use solstone_core_journal_io::operational_log::fold_oplogs;
    use solstone_core_maintenance::registry::routines;
    use solstone_core_maintenance::schedule_sync::{
        expected_entry, host_unscheduled, schedule_name,
    };
    use solstone_core_system::partition::canonical_journal_command;
    use solstone_core_system::work_retry::{self, Unit};

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if ready() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(ready(), "condition did not become ready within 10 seconds");
    }

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn journal_fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let payload = temp.path().join("journal-payload");
        fs::create_dir_all(payload.join("solstone/talent")).unwrap();
        fs::create_dir_all(payload.join("solstone/apps")).unwrap();
        fs::create_dir_all(payload.join("solstone/think/templates")).unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true }
            })
            .to_string(),
        )
        .unwrap();
        (temp, payload)
    }

    fn open_host(journal: &Path, payload: &Path, now: DateTimeUtc) -> ForegroundProcessingHost {
        ForegroundProcessingHost::open_at(
            journal,
            payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            BackupWebDeps::new(journal.to_path_buf()),
        )
        .unwrap()
    }

    #[test]
    fn optional_daily_observation_reports_real_held_discovery_and_closes_on_stop() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for enabled in [false, true] {
            let (temp, payload) = journal_fixture();
            fs::write(
                temp.path().join("config/journal.json"),
                json!({
                    "identity":{"timezone":"UTC"},
                    "processing":{"history_confirmed":true},
                    "providers":{"active":{"provider":"local"}}
                })
                .to_string(),
            )
            .unwrap();
            hold_analysis(temp.path(), true);
            let config = fs::read(temp.path().join("config/journal.json")).unwrap();
            let (settings, gate) =
                solstone_core_system::processing::policy_at(temp.path(), Utc::now()).unwrap();
            assert!(settings.deferred && !gate.open);
            let day = "20260104";
            fs::create_dir_all(temp.path().join("chronicle").join(day)).unwrap();
            let intent =
                solstone_core_system::work_intent::register(temp.path(), day, true).unwrap();
            let now = ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC);
            let host = ForegroundProcessingHost::open_bound_observed(
                Arc::new(JournalRoot::open(temp.path()).unwrap()),
                &payload,
                now,
                None,
                Arc::new(|| false),
                Arc::new(AtomicBool::new(false)),
                BackupWebDeps::new(temp.path().to_path_buf()),
                true,
                enabled,
            )
            .unwrap();
            host.discovery.lock().unwrap().as_ref().unwrap().request();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let snapshot = host.queue.collect_status_snapshot(Instant::now());
                if snapshot.queues.get("daily").copied().unwrap_or(0) > 0 {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "actual held discovery did not publish"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            // Join this fixture's discovery pass before inspecting its output;
            // the production observer neither stops discovery nor waits for it.
            if let Some(mut discovery) = host.discovery.lock().unwrap().take() {
                discovery.shutdown_checked().unwrap();
            }
            if enabled {
                let observation = host.daily_queue_observation().unwrap().unwrap();
                let queued = observation["event"]["queued"].as_array().unwrap();
                let exact = queued
                    .iter()
                    .find(|row| row["request_id"] == intent.id)
                    .unwrap();
                assert_eq!(exact["day"], day);
                assert_eq!(
                    exact["command"],
                    json!(canonical_journal_command([
                        "think",
                        "--day",
                        day,
                        "--from-scratch",
                        "--work-intent",
                        &intent.id
                    ]))
                );
                assert_eq!(observation["authorizes_release"], false);
                assert_eq!(observation["current"]["queued"][0]["request_id"], intent.id);
                assert_eq!(observation["current"]["running_or_reserved"], false);
                assert_eq!(observation["current"]["policy"]["deferred"], true);
                assert_eq!(
                    observation["current"]["policy"]["time_window"]["open"],
                    false
                );
                assert_eq!(
                    observation["current"]["native_root"],
                    observation["native_root"]
                );
                assert_eq!(
                    observation["current"]["host_instance"],
                    observation["host_instance"]
                );
            } else {
                assert!(host.daily_queue_observation().unwrap().is_none());
            }
            assert!(
                host.queue
                    .collect_status_snapshot(Instant::now())
                    .tasks
                    .is_empty()
            );
            assert!(host.queue.history().is_empty());
            // Discovery initializes its own daily_inputs configuration. Every
            // pre-existing setting must survive that ordinary owner write.
            let before: Value = serde_json::from_slice(&config).unwrap();
            let after: Value =
                serde_json::from_slice(&fs::read(temp.path().join("config/journal.json")).unwrap())
                    .unwrap();
            for (key, value) in before.as_object().unwrap() {
                assert_eq!(&after[key], value);
            }
            assert_eq!(
                solstone_core_system::work_intent::read(temp.path()).unwrap()[day].id,
                intent.id
            );
            host.request_stop();
            if enabled {
                assert_eq!(host.daily_queue_observation(), Err("observer_stopped"));
            } else {
                assert!(host.daily_queue_observation().unwrap().is_none());
            }
            host.shutdown().unwrap();
        }
    }

    type DateTimeUtc = chrono::DateTime<Utc>;

    #[test]
    fn daily_observation_rechecks_cancellation_after_read() {
        use std::sync::atomic::AtomicUsize;
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        hold_analysis(temp.path(), true);
        let armed = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        let caller = std::thread::current().id();
        let external: Checkpoint = {
            let armed = Arc::clone(&armed);
            let checks = Arc::clone(&checks);
            Arc::new(move || {
                armed.load(Ordering::Acquire)
                    && std::thread::current().id() == caller
                    && checks.fetch_add(1, Ordering::AcqRel) > 0
            })
        };
        let host = ForegroundProcessingHost::open_bound_observed(
            Arc::new(JournalRoot::open(temp.path()).unwrap()),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            None,
            external,
            Arc::new(AtomicBool::new(false)),
            BackupWebDeps::new(temp.path().to_path_buf()),
            true,
            true,
        )
        .unwrap();
        armed.store(true, Ordering::Release);
        assert_eq!(host.daily_queue_observation(), Err("observer_stopped"));
        assert_eq!(checks.load(Ordering::Acquire), 2);
        armed.store(false, Ordering::Release);
        assert_eq!(host.daily_queue_observation(), Err("observer_stopped"));
        host.shutdown().unwrap();
    }

    #[test]
    fn live_observation_binds_each_host_and_refuses_unreadable_policy_and_root_drift() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        hold_analysis(temp.path(), true);
        let mut identities = Vec::new();
        for _ in 0..2 {
            let host = ForegroundProcessingHost::open_bound_observed(
                Arc::new(JournalRoot::open(temp.path()).unwrap()),
                &payload,
                ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
                None,
                Arc::new(|| false),
                Arc::new(AtomicBool::new(false)),
                BackupWebDeps::new(temp.path().to_path_buf()),
                true,
                true,
            )
            .unwrap();
            let value = host.daily_queue_observation().unwrap().unwrap();
            identities.push((value["native_root"].clone(), value["host_instance"].clone()));
            assert_eq!(value["current"]["native_root"], value["native_root"]);
            assert_eq!(value["current"]["host_instance"], value["host_instance"]);
            assert_eq!(value["current"]["atomic_with_policy_or_settings"], false);
            assert_eq!(value["authorizes_release"], false);
            host.shutdown().unwrap();
        }
        assert_eq!(identities[0].0, identities[1].0);
        assert_ne!(identities[0].1, identities[1].1);
        let host = ForegroundProcessingHost::open_bound_observed(
            Arc::new(JournalRoot::open(temp.path()).unwrap()),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            None,
            Arc::new(|| false),
            Arc::new(AtomicBool::new(false)),
            BackupWebDeps::new(temp.path().to_path_buf()),
            true,
            true,
        )
        .unwrap();
        if let Some(mut discovery) = host.discovery.lock().unwrap().take() {
            discovery.shutdown_checked().unwrap();
        }
        let config = temp.path().join("config/journal.json");
        let bytes = fs::read(&config).unwrap();
        fs::write(&config, b"{").unwrap();
        assert_eq!(
            host.daily_queue_observation(),
            Err("observer_policy_unavailable")
        );
        fs::write(config, bytes).unwrap();
        let retained = temp.path().with_extension("observer-retained");
        fs::rename(temp.path(), &retained).unwrap();
        fs::create_dir(temp.path()).unwrap();
        assert_eq!(host.daily_queue_observation(), Err("observer_stopped"));
        fs::remove_dir(temp.path()).unwrap();
        fs::rename(retained, temp.path()).unwrap();
        assert_eq!(host.daily_queue_observation(), Err("observer_stopped"));
        host.shutdown().unwrap();
    }

    #[test]
    fn live_observation_counts_delay_in_its_final_root_checkpoint() {
        use std::sync::atomic::AtomicUsize;
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        hold_analysis(temp.path(), true);
        let armed = Arc::new(AtomicBool::new(false));
        let checks = Arc::new(AtomicUsize::new(0));
        let caller = std::thread::current().id();
        let external: Checkpoint = {
            let armed = Arc::clone(&armed);
            let checks = Arc::clone(&checks);
            Arc::new(move || {
                if armed.load(Ordering::Acquire)
                    && std::thread::current().id() == caller
                    && checks.fetch_add(1, Ordering::AcqRel) == 2
                {
                    std::thread::sleep(Duration::from_millis(5100));
                }
                false
            })
        };
        let host = ForegroundProcessingHost::open_bound_observed(
            Arc::new(JournalRoot::open(temp.path()).unwrap()),
            &payload,
            ScheduleNow::in_zone(Utc::now(), chrono_tz::UTC),
            None,
            external,
            Arc::new(AtomicBool::new(false)),
            BackupWebDeps::new(temp.path().to_path_buf()),
            true,
            true,
        )
        .unwrap();
        armed.store(true, Ordering::Release);
        assert_eq!(
            host.daily_queue_observation(),
            Err("observer_live_read_stale")
        );
        armed.store(false, Ordering::Release);
        assert_eq!(checks.load(Ordering::Acquire), 3);
        host.shutdown().unwrap();
    }

    #[test]
    fn hosted_wrapper_preserves_native_seal_failure_before_discarding_stderr() {
        use sha2::{Digest, Sha256};
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let day = "20261016";
        fs::create_dir_all(temp.path().join("chronicle").join(day)).unwrap();
        let identity = solstone_core_think_cli::ActivityRetry {
            day: day.into(),
            facet: "home".into(),
            activity: "held-writing".into(),
        };
        let bytes = serde_json::to_vec(&identity).unwrap();
        let pending = temp
            .path()
            .join("health/activity-work")
            .join(format!("{:x}.json", Sha256::digest(bytes)));
        fs::create_dir_all(pending.parent().unwrap()).unwrap();
        fs::write(
            &pending,
            json!({"version":1,"identity":identity,"input_hash":"retained",
            "remaining":["work"],"uses":{},"attempts":0,"next_attempt_ms":0})
            .to_string(),
        )
        .unwrap();
        let original = fs::read(&pending).unwrap();
        let request = solstone_core_system::work_intent::register(temp.path(), day, true).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = ForegroundProcessingHost::open_bound(
            Arc::new(JournalRoot::open(temp.path()).unwrap()),
            &payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            None,
            Arc::new(|| false),
            Arc::new(AtomicBool::new(false)),
            BackupWebDeps::new(temp.path().to_path_buf()),
            true,
        )
        .unwrap();
        let code = host.execute_worker_command(&owned(&[
            "think",
            "--day",
            day,
            "--from-scratch",
            "--work-intent",
            &request.id,
        ]));
        assert_ne!(code, 0);
        let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").unwrap();
        let events = fold_oplogs(
            JournalRoot::open(temp.path()).unwrap(),
            &[date],
            |events: &mut Vec<Value>, entry, input| {
                if entry.name().source().display_slug() == "think" {
                    for line in std::io::BufRead::lines(std::io::BufReader::new(input)) {
                        let line = line?;
                        events.push(serde_json::from_str(&line).unwrap());
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        let complete = events
            .iter()
            .find(|e| e["event"] == "run.complete")
            .expect("native closing outcome survives wrapper");
        assert!(complete["failed"].as_u64().unwrap() > 0);
        let expected = json!({"stage":"rebuild_segment_seal","detail":"requested day has unfinished activity work"});
        assert!(
            complete["failure_details"]
                .as_array()
                .unwrap()
                .contains(&expected),
            "{events:?}"
        );
        assert!(events.iter().any(|e| {
            e["event"] == "phase.complete"
                && e["failure_details"]
                    .as_array()
                    .is_some_and(|details| details.contains(&expected))
        }));
        assert_eq!(fs::read(pending).unwrap(), original);
        assert!(
            solstone_core_system::work_intent::read(temp.path()).unwrap()[day]
                .rebuild
                .as_ref()
                .unwrap()
                .segments
                .is_none()
        );
    }

    fn quiet_entries() -> Map<String, Value> {
        let mut entries = Map::new();
        for (name, tail, every, max_runtime) in [
            ("heartbeat", &["heartbeat"][..], "daily", "10m"),
            (
                "weekly-agents",
                &["think", "--weekly", "-v"][..],
                "weekly",
                "30m",
            ),
            ("cadence", &["think", "--cadence"][..], "5m", "10m"),
            ("brain", &["brain", "refresh"][..], "daily", "5m"),
            (
                "facet-candidates",
                &["facet-candidates"][..],
                "weekly",
                "10m",
            ),
            (
                "rebuild-edges",
                &["indexer", "--rebuild-edges"][..],
                "weekly",
                "10m",
            ),
        ] {
            entries.insert(
                name.to_owned(),
                json!({
                    "cmd": canonical_journal_command(tail.iter().copied()),
                    "every": every,
                    "enabled": false,
                    "max_runtime": max_runtime,
                }),
            );
        }
        for descriptor in routines() {
            let mut entry = expected_entry(descriptor);
            entry["enabled"] = json!(false);
            entries.insert(schedule_name(descriptor.id), entry);
        }
        entries
    }

    fn write_schedules(journal: &Path, entries: Map<String, Value>) {
        let mut raw = Map::new();
        raw.insert("daily_time".into(), json!("00:15"));
        raw.insert("weekly_time".into(), json!("03:15"));
        raw.extend(entries);
        fs::write(
            journal.join("config/schedules.json"),
            serde_json::to_vec_pretty(&Value::Object(raw)).unwrap(),
        )
        .unwrap();
    }

    fn write_scheduler_state(journal: &Path, state: Value) {
        fs::create_dir_all(journal.join("health")).unwrap();
        fs::write(
            journal.join("health/scheduler.json"),
            serde_json::to_vec_pretty(&state).unwrap(),
        )
        .unwrap();
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn scheduler_field(journal: &Path, name: &str, field: &str) -> Option<String> {
        let path = journal.join("health/scheduler.json");
        let bytes = fs::read(&path).ok()?;
        serde_json::from_slice::<Value>(&bytes)
            .ok()?
            .get(name)?
            .get(field)?
            .as_str()
            .map(str::to_owned)
    }

    fn heartbeat_events(journal: &Path, event: &str) -> usize {
        let root = JournalRoot::open(journal).unwrap();
        let today = chrono::Local::now().date_naive();
        let previous = today.pred_opt().unwrap();
        fold_oplogs(
            root,
            &[previous, today],
            |count: &mut usize, entry, input| {
                if entry.name().source().display_slug() != "heartbeat"
                    || entry.name().run().display_slug() != "pass"
                {
                    return Ok(());
                }
                for line in std::io::BufRead::lines(std::io::BufReader::new(input)) {
                    let line = line?;
                    let Ok(value) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if value.get("event").and_then(Value::as_str) == Some(event) {
                        *count += 1;
                    }
                }
                Ok(())
            },
        )
        .unwrap()
    }

    #[test]
    fn routed_commands_match_the_closed_set() {
        let root = tempfile::tempdir().unwrap();
        let journal = root.path();
        assert!(!MAINTENANCE_ROUTINE_IDS.is_empty());
        let admitted = [
            &["think", "--cadence"][..],
            &["think", "--settle", "--stream", "notes"][..],
            &[
                "think",
                "--day",
                "20261012",
                "--segment",
                "120000_1",
                "--stream",
                "notes",
            ][..],
            &[
                "think",
                "--day",
                "20261012",
                "--segment",
                "120000_1",
                "--stream",
                "notes",
                "--repair-segment",
            ][..],
            &[
                "think",
                "--day",
                "20261012",
                "--activity",
                "stand",
                "--facet",
                "work",
            ][..],
            &["think", "--day", "20261012", "--daily-only"][..],
            &[
                "think",
                "--day",
                "20261012",
                "--from-scratch",
                "--work-intent",
                "abc",
            ][..],
            &["think", "--weekly"][..],
            &["think", "--weekly", "-v"][..],
            &["brain", "refresh"][..],
            &["heartbeat"][..],
            &["facet-candidates"][..],
            &["indexer", "--rebuild-edges"][..],
        ];
        for args in admitted {
            assert!(admit_command(journal, &owned(args)), "{args:?}");
            let mut prefixed = vec!["solstone".to_owned(), "journal".to_owned()];
            prefixed.extend(owned(args));
            assert!(admit_command(journal, &prefixed), "{args:?}");
        }
        for id in MAINTENANCE_ROUTINE_IDS {
            assert!(admit_command(journal, &owned(&["maintenance", "run", id])));
        }
        let daily = Unit::Daily {
            day: "20261012".into(),
            talent: "summary".into(),
            facet: None,
        };
        work_retry::observe_held(journal, &daily, "schema_invalid", "input").unwrap();
        work_retry::request(journal, &daily).unwrap();
        let daily_id = daily.id().unwrap();
        assert!(admit_command(
            journal,
            &owned(&[
                "think",
                "--day",
                "20261012",
                "--retry-unit",
                &daily_id,
                "--retry-cause",
                "owner:unused",
            ])
        ));
        let raw = Unit::Raw {
            day: "20261012".into(),
            stream: "notes".into(),
            segment: "120000_1".into(),
            handler: "describe".into(),
            output: "screen.jsonl".into(),
            direct: false,
        };
        work_retry::observe_held(journal, &raw, "generate-wire-failed", "screen.jsonl").unwrap();
        assert!(admit_command(
            journal,
            &owned(&[
                "think",
                "--day",
                "20261012",
                "--retry-unit",
                &raw.id().unwrap(),
                "--retry-cause",
                "input",
            ])
        ));
        for args in [
            &["maintenance", "run", "backup:offload"][..],
            &["think", "--day", "20261012", "--refresh"][..],
            &["think", "--weekly", "--refresh"][..],
            &["think", "--segments"][..],
            &["describe"][..],
            &["describe", "sample.png"][..],
            &["think", "--day", "20261012"][..],
            &["think", "--day", "20261012", "--from-scratch"][..],
            &[
                "think",
                "--day",
                "20261012",
                "--from-scratch",
                "--work-intent",
                "",
            ][..],
            &[
                "think",
                "--day",
                "20261012",
                "--retry-unit",
                "missing",
                "--retry-cause",
                "owner:unused",
            ][..],
        ] {
            assert!(!admit_command(journal, &owned(args)), "{args:?}");
        }
    }

    #[test]
    fn direct_commands_cover_heartbeat_recency_and_empty_skips() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        assert_eq!(host.execute_worker_command(&owned(&["heartbeat"])), 0);
        assert_eq!(heartbeat_events(temp.path(), "pass"), 1);
        assert_eq!(heartbeat_events(temp.path(), "pass.outcome"), 1);
        assert_eq!(host.execute_worker_command(&owned(&["heartbeat"])), 0);
        assert_eq!(heartbeat_events(temp.path(), "pass"), 1);
        assert_eq!(heartbeat_events(temp.path(), "pass.outcome"), 1);
        assert_eq!(
            host.execute_worker_command(&owned(&["facet-candidates"])),
            0
        );
        assert!(!temp.path().join("facets/review-candidates.jsonl").exists());
        assert_eq!(
            host.execute_worker_command(&owned(&["maintenance", "run", "health:mark-raw"])),
            0
        );
        assert_eq!(
            host.execute_worker_command(&owned(&[
                "maintenance",
                "run",
                "speakers:discover-voices"
            ])),
            0
        );
    }

    #[test]
    fn unsupported_offload_does_not_complete() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let mut entries = quiet_entries();
        let name = schedule_name("backup:offload");
        entries.get_mut(&name).unwrap()["enabled"] = json!(true);
        entries.insert(
            "custom-shell".into(),
            json!({
                "cmd": ["echo", "divergent"],
                "every": "daily",
                "enabled": true,
                "max_runtime": "1m",
            }),
        );
        entries.insert("broken".into(), json!("not-an-entry"));
        entries.get_mut("heartbeat").unwrap()["enabled"] = json!(true);
        write_schedules(temp.path(), entries);
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        let schedules = read_json(&temp.path().join("config/schedules.json"));
        let removed = host_unscheduled().contains(&"backup:offload");
        if removed {
            assert!(schedules.get(&name).is_none());
        } else {
            assert_eq!(schedules[&name]["enabled"], json!(true));
            wait_until(|| {
                scheduler_field(temp.path(), &name, "last_status")
                    == Some("uncompleted-hold".into())
            });
            assert_ne!(
                scheduler_field(temp.path(), &name, "last_status").as_deref(),
                Some("success")
            );
            let hold = host
                .queue()
                .history()
                .into_iter()
                .find(|record| record.scheduler_name.as_deref() == Some(name.as_str()))
                .expect("offload hold row");
            assert_ne!(hold.exit_status, "ok");
            assert_eq!(hold.scheduler_name.as_deref(), Some(name.as_str()));
        }
        assert_eq!(
            schedules["custom-shell"]["cmd"],
            json!(["echo", "divergent"])
        );
        assert_eq!(schedules["broken"], json!("not-an-entry"));
        wait_until(|| {
            scheduler_field(temp.path(), "custom-shell", "last_status")
                == Some("uncompleted-hold".into())
                && scheduler_field(temp.path(), "heartbeat", "last_status")
                    == Some("success".into())
        });
        assert_ne!(
            scheduler_field(temp.path(), "custom-shell", "last_status").as_deref(),
            Some("success")
        );
        let shell = host
            .queue()
            .history()
            .into_iter()
            .find(|record| record.scheduler_name.as_deref() == Some("custom-shell"))
            .expect("shell hold row");
        assert_ne!(shell.exit_status, "ok");
        assert_eq!(heartbeat_events(temp.path(), "pass.outcome"), 1);
        drop(host);
    }

    #[test]
    fn second_open_runs_a_due_entry_and_skips_a_name_added_then() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let start = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let mut entries = quiet_entries();
        entries.insert(
            "heartbeat".into(),
            json!({
                "cmd": canonical_journal_command(["heartbeat"]),
                "every": "5m",
                "enabled": true,
                "max_runtime": "10m",
            }),
        );
        write_schedules(temp.path(), entries);
        write_scheduler_state(
            temp.path(),
            json!({"heartbeat": {"last_run": start.timestamp() as f64}}),
        );
        let first = open_host(temp.path(), &payload, start);
        assert!(scheduler_field(temp.path(), "heartbeat", "last_status").is_none());
        drop(first);
        let mut schedules = read_json(&temp.path().join("config/schedules.json"));
        schedules.as_object_mut().unwrap().remove("cadence");
        fs::write(
            temp.path().join("config/schedules.json"),
            serde_json::to_vec_pretty(&schedules).unwrap(),
        )
        .unwrap();
        let later = start + chrono::Duration::minutes(6);
        let second = open_host(temp.path(), &payload, later);
        wait_until(|| {
            scheduler_field(temp.path(), "heartbeat", "last_status") == Some("success".into())
        });
        let reference = scheduler_field(temp.path(), "heartbeat", "last_ref").unwrap();
        assert!(reference.starts_with("sched:heartbeat:"));
        assert!(scheduler_field(temp.path(), "cadence", "last_status").is_none());
        let schedules = read_json(&temp.path().join("config/schedules.json"));
        assert!(schedules.get("cadence").is_some());
        assert_eq!(
            schedules["maintenance:speakers:consolidate-pool"]["enabled"],
            json!(false)
        );
        drop(second);
    }

    #[test]
    fn backup_and_speaker_discovery_record_full_names() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let mut entries = quiet_entries();
        for id in ["backup:run", "speakers:discover-voices"] {
            let name = schedule_name(id);
            entries.get_mut(&name).unwrap()["enabled"] = json!(true);
        }
        write_schedules(temp.path(), entries);
        solstone_core_backup::generate_and_store_keys(temp.path()).unwrap();
        solstone_core_backup::set_destination(
            temp.path(),
            &Destination {
                repository: temp.path().join("backup-repo").display().to_string(),
                backend: "s3".into(),
                credentials: Map::new(),
            },
        )
        .unwrap();
        solstone_core_backup::set_enabled(temp.path(), true).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        wait_until(|| {
            scheduler_field(temp.path(), "maintenance:backup:run", "last_status").is_some()
                && scheduler_field(
                    temp.path(),
                    "maintenance:speakers:discover-voices",
                    "last_status",
                )
                .is_some()
        });
        assert_eq!(
            scheduler_field(
                temp.path(),
                "maintenance:speakers:discover-voices",
                "last_status"
            )
            .as_deref(),
            Some("success")
        );
        assert_eq!(
            scheduler_field(temp.path(), "maintenance:backup:run", "last_status").as_deref(),
            Some("failed")
        );
        assert!(
            scheduler_field(temp.path(), "maintenance:backup:run", "last_ref")
                .unwrap()
                .starts_with("sched:maintenance:backup:run:")
        );
        assert!(
            scheduler_field(
                temp.path(),
                "maintenance:speakers:discover-voices",
                "last_ref"
            )
            .unwrap()
            .starts_with("sched:maintenance:speakers:discover-voices:")
        );
        let history = host.queue().history();
        assert!(history.iter().any(|record| {
            record.scheduler_name.as_deref() == Some("maintenance:backup:run")
                && record.exit_status != "ok"
                && record
                    .reference
                    .starts_with("sched:maintenance:backup:run:")
        }));
        assert!(history.iter().any(|record| {
            record.scheduler_name.as_deref() == Some("maintenance:speakers:discover-voices")
                && record.exit_status == "ok"
                && record
                    .reference
                    .starts_with("sched:maintenance:speakers:discover-voices:")
        }));
        drop(host);
    }

    #[test]
    fn weekly_schedule_succeeds_without_a_reflection_file() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        fs::write(
            temp.path().join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true },
                "providers": { "active": { "provider": "local" } }
            })
            .to_string(),
        )
        .unwrap();
        let mut entries = quiet_entries();
        entries.get_mut("weekly-agents").unwrap()["enabled"] = json!(true);
        write_schedules(temp.path(), entries);
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        wait_until(|| {
            scheduler_field(temp.path(), "weekly-agents", "last_status") == Some("success".into())
        });
        let reflections = temp.path().join("reflections/weekly");
        let pages = fs::read_dir(&reflections)
            .map(|entries| entries.filter_map(|entry| entry.ok()).count())
            .unwrap_or(0);
        assert_eq!(pages, 0);
        drop(host);
    }

    fn enable_only(journal: &Path, name: &str) {
        let mut entries = quiet_entries();
        entries.get_mut(name).unwrap()["enabled"] = json!(true);
        write_schedules(journal, entries);
    }

    #[test]
    fn due_facet_candidates_succeed_without_a_review_file() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        enable_only(temp.path(), "facet-candidates");
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        wait_until(|| {
            scheduler_field(temp.path(), "facet-candidates", "last_status")
                == Some("success".into())
        });
        assert!(!temp.path().join("facets/review-candidates.jsonl").exists());
        drop(host);
    }

    #[test]
    fn due_rebuild_edges_records_the_edge_index() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        enable_only(temp.path(), "rebuild-edges");
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        wait_until(|| {
            scheduler_field(temp.path(), "rebuild-edges", "last_status") == Some("success".into())
        });
        let reference = scheduler_field(temp.path(), "rebuild-edges", "last_ref").unwrap();
        assert!(reference.starts_with("sched:rebuild-edges:"));
        assert!(solstone_core_indexer_store::db::db_path(temp.path()).is_file());
        drop(host);
    }

    #[test]
    fn cancelled_brain_schedule_leaves_no_record() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        enable_only(temp.path(), "brain");
        let start = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        write_scheduler_state(
            temp.path(),
            json!({"brain": {"last_run": start.timestamp() as f64}}),
        );
        let host = open_host(temp.path(), &payload, start);
        assert!(scheduler_field(temp.path(), "brain", "last_status").is_none());
        host.request_stop();
        let later = Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap();
        host.check_schedule(ScheduleNow::in_zone(later, chrono_tz::UTC))
            .unwrap();
        assert!(!temp.path().join("health/brain.json").exists());
        assert!(scheduler_field(temp.path(), "brain", "last_status").is_none());
        assert!(
            host.queue()
                .history()
                .iter()
                .all(|record| record.exit_status != "success")
        );
        drop(host);
    }

    #[test]
    fn discovered_daily_retry_consumes_permission_once() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, _empty_payload) = journal_fixture();
        let payload = checkout_payload();
        fs::write(
            temp.path().join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true },
                "providers": { "active": { "provider": "local" } }
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("facets/work")).unwrap();
        fs::write(
            temp.path().join("facets/work/facet.json"),
            "{\"id\":\"11111111-1111-4111-8111-111111111111\"}\n",
        )
        .unwrap();
        let today = solstone_core_system::daily_coverage::local_day(temp.path(), Utc::now());
        // A ready health-card segment classifies the facet without requiring
        // processing, so the packaged multi-facet review enters coverage.
        let segment = temp
            .path()
            .join("chronicle")
            .join(&today)
            .join("import.apple_health")
            .join("120000_60");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("imported.md"), "card\n").unwrap();
        fs::write(
            segment.join("talents/facets.json"),
            "[{\"facet\":\"work\"}]\n",
        )
        .unwrap();
        let unit = Unit::Daily {
            day: today.clone(),
            talent: "entities:entities_review".into(),
            facet: Some("work".into()),
        };
        let evidence = work_retry::daily::evidence_with_roots(
            temp.path(),
            &unit,
            &payload.join("solstone/talent"),
            &payload.join("solstone/apps"),
        )
        .unwrap();
        assert_ne!(
            evidence.revision_under(&evidence.contract),
            "stale-revision"
        );
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            &today,
            "entities:entities_review",
            Some("work".into()),
        );
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(
            identity.clone(),
            "stale-revision",
            evidence.contract.clone(),
        );
        record.status = solstone_core_journal_io::DailyUnitStatus::Capped;
        record.reason_code = Some("daily_failed".into());
        solstone_core_journal_io::save_daily_unit_record(temp.path(), &record).unwrap();
        write_schedules(temp.path(), quiet_entries());
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        assert!(host.queue().history().is_empty());
        host.deliver_discovery_once();
        let history = host.queue().history();
        assert_eq!(
            history.len(),
            1,
            "status={:?}",
            history.first().map(|record| record.exit_status.as_str())
        );
        let command = &history[0].command;
        let unit_id = unit.id().unwrap();
        assert!(
            command
                .windows(2)
                .any(|pair| { pair[0] == "--retry-unit" && pair[1] == unit_id })
        );
        assert!(command.iter().all(|arg| arg != "--from-scratch"));
        assert!(command.iter().all(|arg| arg != "--work-intent"));
        let cause = command
            .windows(2)
            .find(|pair| pair[0] == "--retry-cause")
            .map(|pair| pair[1].clone())
            .unwrap();
        let stored = work_retry::read(temp.path(), &unit_id).unwrap().unwrap();
        assert!(stored.owner.is_none());
        assert_eq!(
            stored
                .consumed
                .iter()
                .filter(|item| *item == &cause)
                .count(),
            1,
            "cause={cause} consumed={:?} status={} cmd={command:?}",
            stored.consumed,
            history[0].exit_status
        );
        assert_eq!(history[0].exit_status, "ok");
        let unit_record = solstone_core_journal_io::load_daily_unit_record(temp.path(), &identity)
            .unwrap()
            .expect("daily unit");
        assert!(
            matches!(
                unit_record.status,
                solstone_core_journal_io::DailyUnitStatus::Committed
                    | solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput
            ),
            "daily terminal {:?}",
            unit_record.status
        );
        let consumed = stored.consumed.clone();
        host.deliver_discovery_once();
        let again = work_retry::read(temp.path(), &unit_id).unwrap().unwrap();
        assert_eq!(again.consumed, consumed);
        assert_eq!(host.queue().history().len(), 1);
        let again_record = solstone_core_journal_io::load_daily_unit_record(temp.path(), &identity)
            .unwrap()
            .expect("daily unit");
        assert_eq!(again_record.status, unit_record.status);
        drop(host);
    }

    fn checkout_payload() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../core/payload")
    }

    #[test]
    fn packaged_weekly_publishes_a_reflection_the_index_sees() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, _empty_payload) = journal_fixture();
        fs::write(
            temp.path().join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true },
                "providers": { "active": { "provider": "local" } }
            })
            .to_string(),
        )
        .unwrap();
        let payload = checkout_payload();
        let now = Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        assert_eq!(
            host.execute_worker_command(&owned(&["think", "--weekly", "--day", "20260104",])),
            0
        );
        let pages = fs::read_dir(temp.path().join("reflections/weekly"))
            .expect("weekly writer publishes the reflection")
            .filter_map(|entry| entry.ok())
            .count();
        assert!(pages >= 2, "markdown and document");
        let conn = solstone_core_indexer_store::db::open_index_reader(temp.path()).unwrap();
        let indexed: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks WHERE path LIKE 'reflections/weekly/%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(indexed >= 1, "index sees the published reflection");
        drop(host);
    }

    #[test]
    fn due_weekly_schedule_publishes_a_reflection_the_index_sees() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, _empty_payload) = journal_fixture();
        fs::write(
            temp.path().join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true },
                "providers": { "active": { "provider": "local" } }
            })
            .to_string(),
        )
        .unwrap();
        let mut entries = quiet_entries();
        entries.get_mut("weekly-agents").unwrap()["enabled"] = json!(true);
        write_schedules(temp.path(), entries);
        let payload = checkout_payload();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        let mut status = None;
        for _ in 0..2_000_000 {
            status = scheduler_field(temp.path(), "weekly-agents", "last_status");
            if matches!(status.as_deref(), Some("success" | "failed")) {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(status.as_deref(), Some("success"), "status={status:?}");
        assert_eq!(
            weekly_selection_statuses(temp.path()),
            vec!["none".to_owned()]
        );
        drop(host);
    }

    fn week_start_for_today() -> String {
        let today = Utc::now().with_timezone(&chrono_tz::UTC).date_naive();
        let days_since_saturday = today.weekday().num_days_from_sunday() + 1;
        let end = today - chrono::Duration::days(i64::from(days_since_saturday));
        let start = end - chrono::Duration::days(6);
        assert!(end < today);
        assert_eq!(start.weekday(), chrono::Weekday::Sun);
        start.format("%Y%m%d").to_string()
    }

    fn plant_morning_briefing(journal: &Path, day: &str) {
        let day_dir = journal.join("chronicle").join(day);
        fs::create_dir_all(day_dir.join("default/080000_300")).unwrap();
        fs::create_dir_all(day_dir.join("talents")).unwrap();
        fs::write(
            day_dir.join("talents/morning_briefing.json"),
            json!({
                "version": 1,
                "generated_at": "2026-10-12T07:00:00Z",
                "today": {
                    "date": day,
                    "events": [],
                    "reminders": [],
                    "schedule_summary": "Quiet day."
                },
                "yesterday": ["Alpha memory.", "Beta memory."]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn weekly_selection_statuses(journal: &Path) -> Vec<String> {
        let mut statuses = Vec::new();
        let directory = journal.join("reflections/weekly");
        let Ok(entries) = fs::read_dir(&directory) else {
            return statuses;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            statuses.push(
                document["selection"]["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        statuses.sort();
        statuses
    }

    fn weekly_journal(temp: &tempfile::TempDir) -> PathBuf {
        let journal = temp.path().join("journal");
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true },
                "providers": { "active": { "provider": "google", "model": "gemini" } }
            })
            .to_string(),
        )
        .unwrap();
        journal
    }

    fn enable_weekly(journal: &Path, max_runtime: &str) {
        let mut entries = quiet_entries();
        let entry = entries.get_mut("weekly-agents").unwrap();
        entry["enabled"] = json!(true);
        entry["max_runtime"] = json!(max_runtime);
        write_schedules(journal, entries);
    }

    fn wait_weekly_status(journal: &Path) -> Option<String> {
        let mut status = None;
        for _ in 0..2_000_000 {
            status = scheduler_field(journal, "weekly-agents", "last_status");
            if matches!(status.as_deref(), Some("success" | "failed")) {
                break;
            }
            std::thread::yield_now();
        }
        status
    }

    fn selection_stub(dir: &Path) -> PathBuf {
        let response = json!({
            "schema": "solstone-generate-response-v2",
            "id": null,
            "outcome": "generated",
            "text": "{\"selections\":{\"S00\":\"M001\"}}",
            "model": "test-model",
            "usage": {},
            "finish_reason": "stop",
            "thinking": null,
            "schema_validation": null,
            "input_budget": null,
            "request_budget": null,
            "inference": null
        });
        let script = dir.join("weekly-one-shot.sh");
        fs::write(
            dir.join("weekly-one-shot.sh.response"),
            serde_json::to_vec(&response).unwrap(),
        )
        .unwrap();
        fs::write(
            &script,
            "#!/bin/sh\ncat >/dev/null\ncat \"$0.response\"\nprintf '\\n'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        script
    }

    fn selected_reflection(text: &str) -> solstone_core_generate::GenerateResponse {
        use solstone_core_generate::{GenerateResponse, GeneratedResponse};
        GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".to_owned(),
            usage: json!({}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        }))
    }

    #[test]
    fn due_weekly_schedule_publishes_the_selected_memory_the_index_sees() {
        use solstone_core_generate::OneShotClient;

        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let journal = weekly_journal(&temp);
        let start = week_start_for_today();
        plant_morning_briefing(&journal, &start);
        enable_weekly(&journal, "30m");
        let stub = selection_stub(temp.path());
        let payload = checkout_payload();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = ForegroundProcessingHost::open_with_executor(
            &journal,
            &payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            Arc::new(OneShotClient::at_path(stub)),
            BackupWebDeps::new(journal.clone()),
        )
        .unwrap();
        assert_eq!(wait_weekly_status(&journal).as_deref(), Some("success"));
        let document_path = journal.join(format!("reflections/weekly/{start}.json"));
        let document: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
        assert_eq!(document["selection"]["status"], "model");
        assert_eq!(document["memories"][0]["text"], "Beta memory.");
        assert_eq!(document["memories"][0]["source"]["kind"], "briefing");
        assert_eq!(document["memories"][0]["source"]["briefing_day"], start);
        let uri = document["memories"][0]["source"]["uri"].as_str().unwrap();
        assert!(uri.contains(&start));
        assert!(uri.contains("morning_briefing"));
        let conn = solstone_core_indexer_store::db::open_index_reader(&journal).unwrap();
        let indexed: String = conn
            .query_row(
                "SELECT path FROM chunks WHERE path LIKE 'reflections/weekly/%' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(indexed.contains(&start));
        drop(host);
    }

    #[test]
    fn weekly_stop_during_generate_publishes_neither_reflection_nor_receipt() {
        use solstone_core_generate::{ClientError, GenerateRequest};
        use std::sync::atomic::AtomicBool;

        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let journal = weekly_journal(&temp);
        let start = week_start_for_today();
        plant_morning_briefing(&journal, &start);
        enable_weekly(&journal, "30m");
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let executor = move |_request: &GenerateRequest| {
            entered_flag.store(true, Ordering::Release);
            while !release_flag.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok::<_, ClientError>(selected_reflection(r#"{"selections":{"S00":"M001"}}"#))
        };
        let payload = checkout_payload();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = ForegroundProcessingHost::open_with_executor(
            &journal,
            &payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            Arc::new(executor),
            BackupWebDeps::new(journal.clone()),
        )
        .unwrap();
        wait_until(|| entered.load(Ordering::Acquire));
        host.request_stop();
        release.store(true, Ordering::Release);
        wait_until(|| !host.queue().history().is_empty());
        assert!(weekly_selection_statuses(&journal).is_empty());
        assert!(scheduler_field(&journal, "weekly-agents", "last_status").is_none());
        drop(host);
    }

    #[test]
    fn weekly_root_replacement_during_generate_publishes_on_neither_root() {
        use solstone_core_generate::{ClientError, GenerateRequest};
        use std::sync::atomic::AtomicBool;

        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let journal = weekly_journal(&temp);
        let start = week_start_for_today();
        plant_morning_briefing(&journal, &start);
        enable_weekly(&journal, "30m");
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let executor = move |_request: &GenerateRequest| {
            entered_flag.store(true, Ordering::Release);
            while !release_flag.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok::<_, ClientError>(selected_reflection(r#"{"selections":{"S00":"M001"}}"#))
        };
        let payload = checkout_payload();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = ForegroundProcessingHost::open_with_executor(
            &journal,
            &payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            Arc::new(executor),
            BackupWebDeps::new(journal.clone()),
        )
        .unwrap();
        wait_until(|| entered.load(Ordering::Acquire));
        let parked = temp.path().join("parked");
        fs::rename(&journal, &parked).unwrap();
        fs::create_dir(&journal).unwrap();
        release.store(true, Ordering::Release);
        wait_until(|| !host.queue().history().is_empty());
        assert!(weekly_selection_statuses(&parked).is_empty());
        assert!(weekly_selection_statuses(&journal).is_empty());
        assert!(scheduler_field(&parked, "weekly-agents", "last_status").is_none());
        assert!(scheduler_field(&journal, "weekly-agents", "last_status").is_none());
        drop(host);
    }

    #[test]
    fn weekly_deadline_during_generate_does_not_publish_the_reflection() {
        use solstone_core_generate::{ClientError, GenerateRequest};
        use std::sync::atomic::AtomicBool;
        use std::time::Instant;

        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let journal = weekly_journal(&temp);
        let start = week_start_for_today();
        plant_morning_briefing(&journal, &start);
        enable_weekly(&journal, "8s");
        let entered = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let executor = move |_request: &GenerateRequest| {
            let started = Instant::now();
            entered_flag.store(true, Ordering::Release);
            while started.elapsed() < std::time::Duration::from_secs(12) {
                std::thread::yield_now();
            }
            Ok::<_, ClientError>(selected_reflection(r#"{"selections":{"S00":"M001"}}"#))
        };
        let payload = checkout_payload();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = ForegroundProcessingHost::open_with_executor(
            &journal,
            &payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            Arc::new(executor),
            BackupWebDeps::new(journal.clone()),
        )
        .unwrap();
        let wait_until = Instant::now() + std::time::Duration::from_secs(25);
        while Instant::now() < wait_until && host.queue().history().is_empty() {
            std::thread::yield_now();
        }
        assert!(entered.load(Ordering::Acquire));
        assert!(!host.queue().history().is_empty());
        assert!(weekly_selection_statuses(&journal).is_empty());
        assert_ne!(
            scheduler_field(&journal, "weekly-agents", "last_status").as_deref(),
            Some("success")
        );
        drop(host);
    }

    fn awareness_files(journal: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        let directory = journal.join("awareness");
        let Ok(entries) = fs::read_dir(&directory) else {
            return files;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                files.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(&path).unwrap_or_default(),
                );
            }
        }
        files
    }

    #[test]
    fn backfill_pool_busy_returns_uncompleted_hold() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        enable_only(temp.path(), "maintenance:speakers:backfill-pool");
        fs::create_dir_all(temp.path().join("awareness")).unwrap();
        let progress = temp
            .path()
            .join("awareness/speaker_candidates_backfill.json");
        let _held = solstone_core_journal_io::hold_lock(
            &progress,
            solstone_core_journal_io::LockOptions::default(),
        )
        .unwrap();
        let before = awareness_files(temp.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        let mut status = None;
        for _ in 0..1_000_000 {
            status = scheduler_field(
                temp.path(),
                "maintenance:speakers:backfill-pool",
                "last_status",
            );
            if matches!(
                status.as_deref(),
                Some("uncompleted-hold" | "success" | "failed")
            ) {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(
            status.as_deref(),
            Some("uncompleted-hold"),
            "status={status:?}"
        );
        assert_eq!(awareness_files(temp.path()), before);
        drop(host);
    }

    #[test]
    fn backfill_pool_empty_returns_success() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        enable_only(temp.path(), "maintenance:speakers:backfill-pool");
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        let mut status = None;
        for _ in 0..1_000_000 {
            status = scheduler_field(
                temp.path(),
                "maintenance:speakers:backfill-pool",
                "last_status",
            );
            if matches!(status.as_deref(), Some("success" | "failed")) {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(status.as_deref(), Some("success"), "status={status:?}");
        drop(host);
    }

    #[test]
    fn bound_cold_or_reconnected_journal_persists_stop_without_a_running_host() {
        let (temp, _) = journal_fixture();
        let first =
            solstone_core_system::work_intent::register(temp.path(), "20260101", true).unwrap();
        let root = JournalRoot::open(temp.path()).unwrap();
        persist_owner_stop(&root).unwrap();
        assert!(
            solstone_core_system::work_intent::read(temp.path()).unwrap()["20260101"].owner_stopped
        );
        assert!(
            solstone_core_system::work_intent::begin_rebuild(
                temp.path(),
                "20260101",
                &first.id,
                &root.identity().observation_id()
            )
            .is_err()
        );
        let second =
            solstone_core_system::work_intent::register(temp.path(), "20260101", true).unwrap();
        drop(root);
        let rebound = JournalRoot::open(temp.path()).unwrap();
        persist_owner_stop(&rebound).unwrap();
        let stored = solstone_core_system::work_intent::read(temp.path()).unwrap();
        assert_eq!(stored["20260101"].id, second.id);
        assert!(stored["20260101"].owner_stopped);
        fs::write(temp.path().join("health/work-intent.json"), "{").unwrap();
        assert!(persist_owner_stop(&rebound).is_err());
    }

    #[test]
    fn queued_from_scratch_is_claimed_and_finished() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        fs::write(
            temp.path().join("config/journal.json"),
            json!({
                "identity": { "timezone": "UTC" },
                "processing": { "history_confirmed": true },
                "providers": { "active": { "provider": "local" } }
            })
            .to_string(),
        )
        .unwrap();
        let day = "20260104";
        fs::create_dir_all(temp.path().join("chronicle").join(day)).unwrap();
        let intent = solstone_core_system::work_intent::register(temp.path(), day, true).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        host.deliver_discovery_once();
        wait_until(|| {
            solstone_core_system::work_intent::read(temp.path())
                .unwrap()
                .get(day)
                .is_none_or(|stored| stored.id != intent.id)
        });
        let stored = solstone_core_system::work_intent::read(temp.path()).unwrap();
        assert!(
            stored.get(day).is_none_or(|item| item.id != intent.id),
            "owner finish removes a successful from-scratch intent: {stored:?}"
        );
        drop(host);
    }

    #[test]
    fn cancelled_rebuild_retains_execution_until_generate_and_publication_drain() {
        use solstone_core_generate::{ClientError, GenerateRequest};
        use solstone_core_system::work_intent;
        use std::sync::mpsc;

        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let journal = temp.path();
        let day = "20260104";
        fs::create_dir_all(journal.join("chronicle").join(day)).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            json!({
                "identity": {"timezone": "UTC"},
                "processing": {"history_confirmed": true},
                "providers": {"active": {"provider": "local"}}
            })
            .to_string(),
        )
        .unwrap();
        write_schedules(journal, quiet_entries());
        fs::write(
            payload.join("solstone/talent/morning_briefing.md"),
            concat!(
                "{\n\"type\":\"generate\",\"max_output_tokens\":1024,",
                "\"schedule\":\"daily\",\"priority\":10,\"output\":\"json\",",
                "\"hook\":{\"pre\":\"morning_briefing\"},",
                "\"load\":{\"transcripts\":false}\n}\nWrite the morning briefing."
            ),
        )
        .unwrap();
        let intent = work_intent::register(journal, day, true).unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let executor = move |_request: &GenerateRequest| {
            let _ = entered_tx.send(());
            // A bounded fake transport still returns a successful response after
            // cancellation, exercising the real callable publication boundary.
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30));
            Ok::<_, ClientError>(selected_reflection(r#"{"summary":"Late result"}"#))
        };
        let root = Arc::new(JournalRoot::open(journal).unwrap());
        let binding = root.identity().observation_id();
        let now = Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap();
        let host = Arc::new(
            ForegroundProcessingHost::open_bound(
                root,
                &payload,
                ScheduleNow::in_zone(now, chrono_tz::UTC),
                Some(Arc::new(executor)),
                Arc::new(|| false),
                Arc::new(AtomicBool::new(false)),
                BackupWebDeps::new(journal.to_path_buf()),
                true,
            )
            .unwrap(),
        );
        host.queue().submit(ExecutionRequest::Bus(BusTaskRequest {
            cmd: TaskArgv::from_wire(canonical_journal_command([
                "think",
                "--day",
                day,
                "--from-scratch",
                "--work-intent",
                &intent.id,
            ]))
            .unwrap(),
            reference: "blocked-rebuild".into(),
            day: Some(day.into()),
            scheduler_name: None,
            queue_if_active_cmd_differs: false,
            priority: None,
            daily_catchup_provenance: None,
        }));
        let entered = entered_rx.recv_timeout(Duration::from_secs(10)).is_ok();
        host.request_stop();
        let stored = work_intent::read(journal).unwrap();
        let cannot_resume = !work_intent::can_resume(journal, day, &stored[day]).unwrap();
        let cannot_claim = work_intent::begin_rebuild(journal, day, &intent.id, &binding).is_err();
        let cannot_replace = work_intent::register(journal, day, true).is_err();
        let (drained_tx, drained_rx) = mpsc::channel();
        let draining_host = Arc::clone(&host);
        let drain = std::thread::spawn(move || {
            let result = draining_host.shutdown();
            let _ = drained_tx.send(result);
        });
        let drain_waited = drained_rx.recv_timeout(Duration::from_millis(100)).is_err();
        let reopen_refused = ForegroundProcessingHost::open_at(
            journal,
            &payload,
            ScheduleNow::in_zone(now, chrono_tz::UTC),
            BackupWebDeps::new(journal.to_path_buf()),
        )
        .is_err();
        let _ = release_tx.send(());
        let drained = drained_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drain.join().unwrap();
        assert!(entered, "the real rebuild must reach Generate");
        assert!(cannot_resume && cannot_claim && cannot_replace);
        assert!(drain_waited && reopen_refused);
        drained.unwrap();
        let identity =
            solstone_core_journal_io::DailyUnitIdentity::new(day, "morning_briefing", None);
        let record = solstone_core_journal_io::load_daily_unit_record(journal, &identity)
            .unwrap()
            .unwrap();
        assert!(!record.status.is_terminal_success());
        assert!(
            !journal
                .join(format!("chronicle/{day}/talents/morning_briefing.json"))
                .exists()
        );
        let stored = work_intent::read(journal).unwrap();
        assert_eq!(stored[day].id, intent.id);
        assert!(
            !stored[day].owner_stopped,
            "generic cancellation is not owner Stop"
        );
        let replacement = work_intent::register(journal, day, true).unwrap();
        assert_ne!(replacement.id, intent.id);
        drop(host);
        drop(open_host(journal, &payload, now));
    }

    #[test]
    fn cancelled_queued_from_scratch_stays_claimed() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let day = "20260104";
        fs::create_dir_all(temp.path().join("chronicle").join(day)).unwrap();
        let intent = solstone_core_system::work_intent::register(temp.path(), day, true).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap();
        let host = std::sync::Arc::new(open_host(temp.path(), &payload, now));
        let journal = temp.path().to_path_buf();
        let intent_id = intent.id.clone();
        let watcher = {
            let host = std::sync::Arc::clone(&host);
            std::thread::spawn(move || {
                let lock_path = journal
                    .join(
                        solstone_core_journal_io::durability::artifact(
                            solstone_core_journal_io::durability::ArtifactId::WorkIntent,
                        )
                        .path,
                    )
                    .with_extension("lock");
                for _ in 0..1_000_000 {
                    let claimed = solstone_core_system::work_intent::read(&journal)
                        .ok()
                        .and_then(|days| days.get(day).cloned())
                        .is_some_and(|item| {
                            item.id == intent_id
                                && item.state
                                    == solstone_core_system::work_intent::IntentState::Claimed
                        });
                    if claimed {
                        let _held = solstone_core_journal_io::hold_lock(
                            &lock_path,
                            solstone_core_journal_io::LockOptions::default(),
                        );
                        host.request_stop();
                        return;
                    }
                    std::thread::yield_now();
                }
            })
        };
        let code = host.execute_worker_command(&owned(&[
            "think",
            "--day",
            day,
            "--from-scratch",
            "--work-intent",
            &intent.id,
        ]));
        watcher.join().unwrap();
        assert_ne!(code, 0);
        let stored = solstone_core_system::work_intent::read(temp.path())
            .unwrap()
            .remove(day)
            .unwrap();
        assert_eq!(stored.id, intent.id);
        assert_eq!(
            stored.state,
            solstone_core_system::work_intent::IntentState::Claimed
        );
    }

    #[test]
    fn fresh_root_syncs_maintenance_and_runs_due_heartbeat() {
        let _lock = TEST_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (temp, payload) = journal_fixture();
        let now = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let host = open_host(temp.path(), &payload, now);
        let schedules = read_json(&temp.path().join("config/schedules.json"));
        assert!(schedules.get("maintenance:health:prune-logs").is_some());
        assert!(schedules.get("heartbeat").is_some());
        assert_eq!(
            schedules["heartbeat"]["enabled"],
            json!(true),
            "the default heartbeat stays enabled"
        );
        assert_eq!(heartbeat_events(temp.path(), "pass.outcome"), 0);
        assert!(scheduler_field(temp.path(), "heartbeat", "last_status").is_none());
        let due = Utc.with_ymd_and_hms(2026, 10, 18, 2, 0, 0).unwrap();
        let report = host
            .check_schedule(ScheduleNow::in_zone(due, chrono_tz::UTC))
            .unwrap();
        assert!(
            report.submitted.iter().any(|name| name == "heartbeat"),
            "daily boundary dispatches the default heartbeat: {report:?}"
        );
        wait_until(|| heartbeat_events(temp.path(), "pass.outcome") >= 1);
        assert!(
            solstone_core_indexer_store::db::db_path(temp.path()).is_file()
                || heartbeat_events(temp.path(), "pass") >= 1
        );
        drop(host);
    }
}
