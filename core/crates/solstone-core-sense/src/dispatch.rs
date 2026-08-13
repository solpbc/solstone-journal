// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};
use solstone_core_system::process::{ManagedProcess, SpawnOptions, TerminationError};

use crate::beacon::Health;
use crate::config::{
    HANDLERS, no_thinking_engine, processing_deferred, read_config, resolve_concurrency,
    resolve_max_runtime,
};
use crate::events;
use crate::memory::{Admission, SystemMemoryProbe};
use crate::registry::{
    HandlerSpec, command_for, default_registry, match_handler, segment_dir, with_program,
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

pub struct SenseDispatcher {
    journal: PathBuf,
    state: Arc<Mutex<State>>,
    outbound: mpsc::Sender<Outbound>,
    pools: HashMap<&'static str, Mutex<WorkerPool>>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    admission: Admission,
    handler_program: Option<PathBuf>,
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
        Self::new_inner(journal, verbose, debug, outbound, admission, None)
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
            Some(program),
        )
    }

    fn new_inner(
        journal: PathBuf,
        verbose: bool,
        debug: bool,
        outbound: mpsc::Sender<Outbound>,
        admission: Admission,
        handler_program: Option<PathBuf>,
    ) -> Self {
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
        }));
        let mut pools = HashMap::new();
        let mut worker_handles = Vec::new();
        for handler in HANDLERS {
            let workers = resolve_concurrency(&read_config(&journal), handler);
            let mut senders = Vec::with_capacity(workers);
            for _ in 0..workers {
                let (sender, receiver) = mpsc::channel();
                senders.push(sender);
                let state = Arc::clone(&state);
                let outbound = outbound.clone();
                let journal = journal.clone();
                let admission = admission.clone();
                let worker = thread::Builder::new()
                    .name(format!("{handler}-worker"))
                    .spawn(move || {
                        worker(
                            receiver, state, outbound, journal, admission, verbose, debug,
                        )
                    })
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
            admission,
            handler_program,
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
        let mut registry = default_registry(crate::config::describe_per_proc_jobs(
            &config,
            resolve_concurrency(&config, "describe"),
            &self.journal,
        ));
        if let Some(program) = &self.handler_program {
            with_program(&mut registry, program);
        }
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
            drop(state);
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
        complete(
            &self.state,
            &self.outbound,
            &self.journal,
            key,
            None,
            None,
            note,
        );
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
    pub fn stop(&self) {
        self.state.lock().expect("sense state").stopping = true;
    }
    pub fn stop_and_wait(&self) {
        self.stop();
        self.join_workers();
    }
    fn join_workers(&self) {
        let workers = std::mem::take(&mut *self.workers.lock().expect("sense workers"));
        for worker in workers {
            let _ = worker.join();
        }
    }
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
    journal: PathBuf,
    admission: Admission,
    verbose: bool,
    debug: bool,
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
            Ok(job) => run_job(job, &state, &outbound, &journal, &admission, verbose, debug),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn run_job(
    job: Job,
    state: &Arc<Mutex<State>>,
    outbound: &mpsc::Sender<Outbound>,
    journal: &Path,
    admission: &Admission,
    verbose: bool,
    debug: bool,
) {
    let admission_stopping = || {
        let state = state.lock().expect("sense state");
        state.stopping
    };
    let stage = job.item.handler.clone();
    let context = job.item.context.clone();
    let key = context.key.clone();
    let file = job.item.file_path.clone();
    let admitted = admission.wait(
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
    let command = command_for(&job.spec, &file, verbose, debug);
    let reference = chrono::Utc::now().timestamp_millis().to_string();
    let _ = outbound.send(Outbound {
        tract: "observe",
        event: "detected",
        fields: events::detected(journal, &file, &command, &reference, &context),
    });
    let mut environment = BTreeMap::<OsString, OsString>::new();
    environment.insert("SOL_SEGMENT".into(), context.key.segment.clone().into());
    if let Some(observer) = &context.observer {
        environment.insert("OBSERVER_NAME".into(), observer.into());
    }
    if let Some(meta) = &context.meta {
        environment.insert(
            "SEGMENT_META".into(),
            serde_json::to_string(meta).unwrap_or_default().into(),
        );
    }
    environment.insert(
        "SOL_QUEUE_WAIT_MS".into(),
        events::queue_wait_ms(job.item.queued_at).to_string().into(),
    );
    let options = SpawnOptions {
        journal_root: journal.to_path_buf(),
        reference,
        day: Some(context.key.day.clone()),
        sink: None,
        environment,
    };
    let Ok(mut process) = ManagedProcess::spawn(command, options) else {
        complete(
            state,
            outbound,
            journal,
            &key,
            Some(&file),
            Some(format!("{} spawn failed", job.item.handler)),
            None,
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
            complete(state, outbound, journal, &key, Some(&file), None, None);
            process.cleanup();
            return;
        }
        Some(0) => {
            state.lock().expect("sense state").health.success();
            complete(state, outbound, journal, &key, Some(&file), None, None);
            process.cleanup();
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
            reason
        }
    };
    process.cleanup();
    complete(
        state,
        outbound,
        journal,
        &key,
        Some(&file),
        Some(outcome),
        None,
    );
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

fn complete(
    state: &Arc<Mutex<State>>,
    outbound: &mpsc::Sender<Outbound>,
    journal: &Path,
    key: &SegmentKey,
    file: Option<&Path>,
    error: Option<String>,
    note: Option<&str>,
) {
    let completed = {
        let mut state = state.lock().expect("sense state");
        if let Some(file) = file {
            state
                .pending_files
                .retain(|(_, candidate)| candidate != file);
        }
        let empty = {
            let Some(segment) = state.segments.get_mut(key) else {
                return;
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
    if let Some(segment) = completed {
        if !segment.context.batch {
            let health = journal
                .join("chronicle")
                .join(&segment.context.key.day)
                .join("health");
            let _ = std::fs::create_dir_all(&health);
            let _ = std::fs::File::create(health.join("stream.updated"));
        }
        let _ = outbound.send(Outbound {
            tract: "observe",
            event: "observed",
            fields: events::observed(&segment, note),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use serde_json::json;
    use solstone_core_callosum::CallosumEnvelope;
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
        complete(&state, &outbound, temp.path(), &key, None, Some("describe watchdog_timeout; process_tree_not_reaped reason=survived_sigkill survivors=1".into()), None);
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
    fn live_completion_touches_stream_marker_but_batch_does_not() {
        let temp = tempfile::tempdir().expect("journal");
        let (outbound, _receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            segments: HashMap::new(),
            pending_files: HashSet::new(),
            health: Health::default(),
            stopping: false,
        }));
        for (segment, batch) in [("live", false)] {
            let key = SegmentKey {
                day: "20260812".into(),
                stream: Some("default".into()),
                segment: segment.into(),
            };
            state.lock().expect("state").segments.insert(
                key.clone(),
                SegmentState::new(SegmentContext {
                    key: key.clone(),
                    observer: None,
                    batch,
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
            );
        }
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
        );
        assert!(!marker.exists());
    }
}
