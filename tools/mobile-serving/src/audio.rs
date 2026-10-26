// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A bounded foreground processing pass over durable input, with no job ledger.

use serde_json::{Value, json};
#[cfg(test)]
use sha2::{Digest, Sha256};
use solstone_core_transcribe::{
    ConfidentialIoPhase, EmbeddedControl, TranscribeOutcome, discover_audio_files,
    embedded_already_processed, embedded_retry_context, embedded_retryable_failure,
    process_embedded,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

// Keep local inference available without loading its model when the owner has
// chosen confidential transcription. The model is verified before local use.
struct LocalAsr<'a> {
    model: &'a Path,
    scratch: &'a Path,
    loaded: RefCell<Option<journal_mobile_audio::Parakeet>>,
    load_ms: Cell<Option<f64>>,
    native_call: &'a dyn Fn(bool),
}

impl solstone_core_transcribe::EmbeddedAsr for LocalAsr<'_> {
    fn transcribe(&self, samples: &[f32]) -> Result<Value, String> {
        let mut loaded = self.loaded.borrow_mut();
        if loaded.is_none() {
            let started = Instant::now();
            *loaded = Some(journal_mobile_audio::Parakeet::new(
                self.model,
                self.scratch,
            )?);
            self.load_ms
                .set(Some(started.elapsed().as_secs_f64() * 1000.));
        }
        loaded
            .as_ref()
            .unwrap()
            .transcribe_observed(samples, self.native_call)
    }

    fn model_info(&self) -> solstone_core_transcribe::EmbeddedModelInfo {
        journal_mobile_audio::Parakeet::pinned_model_info()
    }
}

struct State {
    stop: AtomicBool,
    cancel: AtomicBool,
    generation: AtomicU64,
    external_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    status: Mutex<Value>,
    recent_failures: Mutex<VecDeque<Value>>,
    wake: Condvar,
}

impl State {
    fn input_failed(&self, generation: u64, input: &Path, error: &str) {
        // Private qualification/status evidence, bounded and process-local.
        // A later input can replace the headline without erasing this attempt.
        let mut failures = self.recent_failures.lock().unwrap();
        if failures.len() == 64 {
            failures.pop_front();
        }
        failures.push_back(json!({"generation":generation,"input":input,"error":error}));
    }

    fn snapshot(&self) -> Value {
        let mut status = self.status.lock().unwrap().clone();
        status["generation"] = json!(self.generation.load(Ordering::Acquire));
        status["cancel_requested"] = json!(
            self.stop.load(Ordering::Acquire)
                || self.cancel.load(Ordering::Acquire)
                || (self.external_cancelled)()
        );
        status["recent_failures"] = json!(self.recent_failures.lock().unwrap().clone());
        status
    }

    fn request_cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.cancel.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    fn cancelled(&self, generation: u64) -> bool {
        (self.external_cancelled)()
            || self.cancel.load(Ordering::Acquire)
            || self.generation.load(Ordering::Acquire) != generation
    }
}

pub(super) struct AudioWorker {
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
    #[cfg(feature = "processing")]
    host_config: Option<HostAudioConfig>,
}

#[cfg(feature = "processing")]
struct HostAudioConfig {
    root: Arc<solstone_core_journal_io::JournalRoot>,
    models: PathBuf,
    model: PathBuf,
    scratch: PathBuf,
    held: Mutex<(u64, HashSet<PathBuf>)>,
}

impl AudioWorker {
    #[cfg(test)]
    pub fn start(
        root: PathBuf,
        models: PathBuf,
        model: PathBuf,
        scratch: PathBuf,
    ) -> Result<Self, String> {
        Self::start_with_cancellation(root, models, model, scratch, Arc::new(|| false))
    }

    pub fn start_with_cancellation(
        root: PathBuf,
        models: PathBuf,
        model: PathBuf,
        scratch: PathBuf,
        external_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, String> {
        let authority =
            solstone_core_journal_io::JournalRoot::open(&root).map_err(|e| e.to_string())?;
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            external_cancelled,
            status: Mutex::new(json!({"state":"starting"})),
            recent_failures: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        });
        let shared = state.clone();
        let thread = thread::Builder::new()
            .name("journal-audio".into())
            .spawn(move || {
                while !shared.stop.load(Ordering::Acquire) && !(shared.external_cancelled)() {
                    if let Err(error) = run(&shared, &authority, &models, &model, &scratch) {
                        *shared.status.lock().unwrap() = json!({"state":"failed","error":error});
                        let status = shared.status.lock().unwrap();
                        let _ = shared
                            .wake
                            .wait_timeout(status, Duration::from_secs(2))
                            .unwrap();
                    }
                }
                if (shared.external_cancelled)() {
                    shared.status.lock().unwrap()["state"] = json!("cancelled");
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            state,
            thread: Some(thread),
            #[cfg(feature = "processing")]
            host_config: None,
        })
    }

    #[cfg(feature = "processing")]
    pub(crate) fn for_host(
        root: Arc<solstone_core_journal_io::JournalRoot>,
        models: PathBuf,
        model: PathBuf,
        scratch: PathBuf,
        external_cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            state: Arc::new(State {
                stop: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                external_cancelled,
                status: Mutex::new(json!({"state":"ready"})),
                recent_failures: Mutex::new(VecDeque::new()),
                wake: Condvar::new(),
            }),
            thread: None,
            host_config: Some(HostAudioConfig {
                root,
                models,
                model,
                scratch,
                held: Mutex::new((0, HashSet::new())),
            }),
        }
    }

    #[cfg(feature = "processing")]
    pub(crate) fn process_host_input(
        &self,
        input: &Path,
        attempt: Option<&solstone_core_system::work_retry::Attempt>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, String> {
        let config = self
            .host_config
            .as_ref()
            .ok_or("audio configuration unavailable")?;
        config
            .root
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let generation = self.state.generation.load(Ordering::Acquire);
        let interrupted = || {
            self.state.cancelled(generation)
                || self.state.stop.load(Ordering::Acquire)
                || cancelled()
                || config.root.revalidate_canonical_binding().is_err()
        };
        if interrupted() {
            return Ok(false);
        }
        let root = config.root.canonical_path();
        let retry = {
            let mut held = config.held.lock().unwrap();
            if held.0 != generation {
                *held = (generation, HashSet::new());
            }
            match gate_input(root, input, &held.1) {
                InputDecision::Skip => return Ok(false),
                InputDecision::Hold(error) => {
                    held.1.insert(input.to_path_buf());
                    drop(held);
                    self.state.input_failed(generation, input, &error);
                    *self.state.status.lock().unwrap() =
                        json!({"state":"failed","input":input,"error":error});
                    return Err(error);
                }
                InputDecision::Process(retry) => retry,
            }
        };
        if attempt.is_some_and(|attempt| retry.as_ref().map(|r| &r.attempt) != Some(attempt)) {
            return Err("audio retry permission changed".into());
        }
        if interrupted() {
            return Ok(false);
        }
        // Hash the accepted recording once. Later checkpoints keep this witness.
        let admitted = match crate::custody::AcceptedInput::capture(root, input, &interrupted) {
            Ok(Some(admitted)) => admitted,
            Ok(None) => return Ok(false),
            Err(error) => {
                if interrupted() || error == crate::custody::CUSTODY_CANCELLED {
                    return Ok(false);
                }
                let mut held = config.held.lock().unwrap();
                if held.0 != generation {
                    *held = (generation, HashSet::new());
                }
                held.1.insert(input.to_path_buf());
                drop(held);
                self.state.input_failed(generation, input, &error);
                *self.state.status.lock().unwrap() =
                    json!({"state":"failed","input":input,"error":error});
                return Err(error);
            }
        };
        // A retained stamp and the custody receipt name the admitted bytes.
        // Checkpoints do not hash the recording again. Loss still cancels
        // before publication, including a deleted original that reappears.
        let owner_original_deleted = Cell::new(false);
        let cancel = || {
            interrupted()
                || admitted_input_lost(&admitted, root, input, owner_original_deleted.get())
        };
        *self.state.status.lock().unwrap() = json!({"state":"processing","input":input});
        let started = Instant::now();
        let native_call = |active| {
            self.state.status.lock().unwrap()["native_call_active"] = json!(active);
        };
        let asr = LocalAsr {
            model: &config.model,
            scratch: &config.scratch,
            loaded: RefCell::new(None),
            load_ms: Cell::new(None),
            native_call: &native_call,
        };
        journal_mobile_audio::install_embedded_ced();
        let result = process_embedded(
            input,
            config.root.canonical_path(),
            &config.models,
            &config.scratch,
            &asr,
            EmbeddedControl {
                cancelled: &cancel,
                owner_original_deleted: &|| owner_original_deleted.set(true),
                service_io: &|phase| {
                    self.state.status.lock().unwrap()["service_io"] = match phase {
                        ConfidentialIoPhase::RequestFlushed => json!("awaiting_response"),
                        ConfidentialIoPhase::ResponseFinished => json!("finished"),
                    };
                },
            },
            retry.as_ref(),
        );
        let cancelled = match &result {
            Ok(_) => false,
            Err(solstone_core_transcribe::TranscribeError::Cancelled) => true,
            Err(_) => cancel(),
        };
        if cancelled {
            self.state.status.lock().unwrap()["state"] = json!("cancelled");
            return Ok(false);
        }
        match result {
            Ok(result) => {
                *self.state.status.lock().unwrap() = json!({
                    "state":"ready",
                    "last":{"input":input,"outcome":format!("{:?}",result.outcome),
                    "transcript":result.transcript_path,
                    "processing_ms":started.elapsed().as_secs_f64()*1000.}
                });
                Ok(result.outcome != TranscribeOutcome::Skipped)
            }
            Err(error) => {
                if !matches!(
                    error,
                    solstone_core_transcribe::TranscribeError::ConfidentialDeferred { .. }
                ) || !embedded_retryable_failure(input)
                {
                    config.held.lock().unwrap().1.insert(input.to_path_buf());
                }
                self.state
                    .input_failed(generation, input, &error.to_string());
                *self.state.status.lock().unwrap() =
                    json!({"state":"failed","input":input,"error":error.to_string()});
                Err(error.to_string())
            }
        }
    }

    pub fn status(&self) -> Value {
        self.state.snapshot()
    }

    pub fn native_call_active(&self) -> Option<bool> {
        self.state
            .status
            .try_lock()
            .ok()?
            .get("native_call_active")
            .and_then(Value::as_bool)
    }
    pub fn cancel(&self) {
        self.state.request_cancel();
    }
    pub fn stop(&self) {
        self.state.stop.store(true, Ordering::Release);
        self.cancel();
    }
    pub fn resume(&self) {
        // A repaired input/model must retry even when failure did not cancel
        // the worker. Advancing also preserves cancellation of old inferences.
        self.state.generation.fetch_add(1, Ordering::AcqRel);
        self.state.cancel.store(false, Ordering::Release);
        self.state.wake.notify_all();
    }
}

impl Drop for AudioWorker {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum InputDecision {
    Skip,
    Hold(String),
    Process(Option<solstone_core_transcribe::RetryContext>),
}

fn gate_input(root: &Path, input: &Path, held: &HashSet<PathBuf>) -> InputDecision {
    let retry = match embedded_retry_context(root, input) {
        Ok(context) => context,
        Err(error) => return InputDecision::Hold(error),
    };
    if held.contains(input) && retry.is_none() {
        return InputDecision::Skip;
    }
    if embedded_already_processed(root, input, retry.as_ref()) {
        return InputDecision::Skip;
    }
    InputDecision::Process(retry)
}

pub(crate) fn decide_input(root: &Path, input: &Path, held: &HashSet<PathBuf>) -> InputDecision {
    match gate_input(root, input, held) {
        InputDecision::Process(retry) => match accepted_input(root, input) {
            Ok(true) => InputDecision::Process(retry),
            Ok(false) => InputDecision::Skip,
            Err(error) => InputDecision::Hold(error),
        },
        decision => decision,
    }
}

fn run(
    state: &State,
    authority: &solstone_core_journal_io::JournalRoot,
    models: &Path,
    model: &Path,
    scratch: &Path,
) -> Result<(), String> {
    authority
        .revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    if (state.external_cancelled)() {
        return Ok(());
    }
    let root = authority.canonical_path();
    let started = Instant::now();
    let native_call = |active| {
        state.status.lock().unwrap()["native_call_active"] = json!(active);
    };
    let asr = LocalAsr {
        model,
        scratch,
        loaded: RefCell::new(None),
        load_ms: Cell::new(None),
        native_call: &native_call,
    };
    let worker_start_ms = started.elapsed().as_secs_f64() * 1000.;
    *state.status.lock().unwrap() = json!({"state":"ready","worker_start_ms":worker_start_ms,"model_load_ms":asr.load_ms.get()});
    let mut held = HashSet::new();
    let mut retry_generation = state.generation.load(Ordering::Acquire);
    while !state.stop.load(Ordering::Acquire) && !(state.external_cancelled)() {
        authority
            .revalidate_canonical_binding()
            .map_err(|e| e.to_string())?;
        let current_generation = state.generation.load(Ordering::Acquire);
        if current_generation != retry_generation {
            held.clear();
            retry_generation = current_generation;
        }
        if state.cancel.load(Ordering::Acquire) {
            let mut status = state.status.lock().unwrap();
            if status["state"] != "failed" {
                status["state"] = json!("cancelled");
            }
        } else {
            let mut day_errors = Vec::new();
            let inputs = discover_audio_files(root, &mut |_| {}, &mut |day, error| {
                day_errors.push(format!("{}: {error}", day.display()))
            })
            .collect::<Vec<_>>();
            let pass_day_error = if !day_errors.is_empty() {
                let err_str = day_errors.join("; ");
                let mut status = state.status.lock().unwrap();
                status["day_error"] = json!(err_str.clone());
                status["error"] = json!(err_str.clone());
                Some(err_str)
            } else {
                let mut status = state.status.lock().unwrap();
                if let Some(previous) = status
                    .as_object_mut()
                    .and_then(|row| row.remove("day_error"))
                    && status.get("error") == Some(&previous)
                {
                    status.as_object_mut().unwrap().remove("error");
                }
                None
            };
            let set_status = |mut obj: serde_json::Value| {
                if let Some(ref err) = pass_day_error {
                    obj["day_error"] = json!(err);
                }
                *state.status.lock().unwrap() = obj;
            };
            for input in inputs {
                if state.stop.load(Ordering::Acquire)
                    || state.cancelled(state.generation.load(Ordering::Acquire))
                {
                    break;
                }
                match decide_input(root, &input, &held) {
                    InputDecision::Skip => continue,
                    InputDecision::Hold(error) => {
                        let generation = state.generation.load(Ordering::Acquire);
                        state.input_failed(generation, &input, &error);
                        held.insert(input.clone());
                        set_status(json!({
                            "state": "failed",
                            "input": input,
                            "error": error,
                            "failed_inputs": held.len(),
                        }));
                        continue;
                    }
                    InputDecision::Process(retry) => {
                        let generation = state.generation.load(Ordering::Acquire);
                        if state.cancelled(generation) {
                            break;
                        }
                        let start = Instant::now();
                        let prior = state.status.lock().unwrap().clone();
                        set_status(json!({
                            "state": "processing",
                            "input": input,
                            "previous": prior.get("last"),
                            "model_load_ms": asr.load_ms.get(),
                        }));
                        journal_mobile_audio::install_embedded_ced();
                        let result = process_embedded(
                            &input,
                            root,
                            models,
                            scratch,
                            &asr,
                            EmbeddedControl {
                                cancelled: &|| {
                                    state.cancelled(generation)
                                        || authority.revalidate_canonical_binding().is_err()
                                },
                                owner_original_deleted: &|| {},
                                service_io: &|phase| {
                                    let mut status = state.status.lock().unwrap();
                                    status["service_io"] = match phase {
                                        ConfidentialIoPhase::RequestFlushed => {
                                            json!("awaiting_response")
                                        }
                                        ConfidentialIoPhase::ResponseFinished => json!("finished"),
                                    };
                                },
                            },
                            retry.as_ref(),
                        );
                        let cancelled = match &result {
                            Ok(_) => false,
                            Err(solstone_core_transcribe::TranscribeError::Cancelled) => true,
                            Err(_) => state.cancelled(generation),
                        };
                        if cancelled {
                            state.status.lock().unwrap()["state"] = json!("cancelled");
                            break;
                        }
                        match result {
                            Ok(result) if result.outcome == TranscribeOutcome::Skipped => {
                                *state.status.lock().unwrap() = prior;
                            }
                            Ok(result) => {
                                let last = json!({
                                    "input": input,
                                    "outcome": format!("{:?}", result.outcome),
                                    "transcript": result.transcript_path,
                                    "processing_ms": start.elapsed().as_secs_f64() * 1000.,
                                    "failed_inputs": held.len(),
                                });
                                set_status(json!({
                                    "state": "ready",
                                    "model_load_ms": asr.load_ms.get(),
                                    "last": last,
                                }));
                            }
                            Err(error) => {
                                if !matches!(error, solstone_core_transcribe::TranscribeError::ConfidentialDeferred { .. })
                                    || !embedded_retryable_failure(&input)
                                {
                                    held.insert(input.clone());
                                }
                                state.input_failed(generation, &input, &error.to_string());
                                set_status(json!({
                                    "state": "failed",
                                    "input": input,
                                    "error": error.to_string(),
                                    "model_load_ms": asr.load_ms.get(),
                                    "failed_inputs": held.len(),
                                }));
                            }
                        }
                    }
                }
            }
        }
        let status = state.status.lock().unwrap();
        let _ = state
            .wake
            .wait_timeout(status, Duration::from_secs(2))
            .unwrap();
    }
    Ok(())
}

fn accepted_input(root: &Path, input: &Path) -> Result<bool, String> {
    crate::custody::accepted_input(root, input)
}

#[cfg(all(feature = "processing", test))]
fn input_custody_lost(root: &Path, input: &Path, owner_original_deleted: bool) -> bool {
    if owner_original_deleted {
        // Retention deleted the admitted original under the publication claim.
        // Recreated bytes cannot inherit that completed deletion's authority.
        !matches!(std::fs::symlink_metadata(input), Err(error)
            if error.kind() == std::io::ErrorKind::NotFound)
    } else {
        !accepted_input(root, input).unwrap_or(false)
    }
}

#[cfg(feature = "processing")]
fn admitted_input_lost(
    admitted: &crate::custody::AcceptedInput,
    root: &Path,
    input: &Path,
    owner_original_deleted: bool,
) -> bool {
    if owner_original_deleted {
        // Retention deleted the admitted original under the publication claim.
        // Recreated bytes cannot inherit that completed deletion's authority.
        !matches!(std::fs::symlink_metadata(input), Err(error)
            if error.kind() == std::io::ErrorKind::NotFound)
    } else {
        // Stamp and receipt only. The admission hash is not repeated.
        !admitted.still_current(input)
            || !crate::custody::receipt_still_names(root, input).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_callosum::{
        DeviceIngestEvent, DurableEvent, FileDescriptor, append_durable_event,
    };
    use solstone_core_transcribe::TranscribeError;
    use std::fs::File;

    fn setup_fixture_audio(
        root: &Path,
        day: &str,
        stream: &str,
        segment_name: &str,
        filename: &str,
        content: &[u8],
    ) -> PathBuf {
        let segment = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment_name);
        std::fs::create_dir_all(&segment).unwrap();
        let audio_path = segment.join(filename);
        std::fs::write(&audio_path, content).unwrap();
        let event = DeviceIngestEvent {
            record_type: "device_ingest".into(),
            record_version: 1,
            outcome: "accepted".into(),
            protocol_version: 3,
            cid: "fixture".into(),
            source: "audio".into(),
            stream: stream.into(),
            day: day.into(),
            segment: segment_name.into(),
            meta: Default::default(),
            extra: Default::default(),
            files: vec![FileDescriptor {
                submitted: filename.into(),
                written: filename.into(),
                size: content.len() as u64,
                sha256: format!("{:x}", Sha256::digest(content)),
                extra: Default::default(),
            }],
        };
        append_durable_event(&segment, &DurableEvent::DeviceIngest(event)).unwrap();
        audio_path
    }

    #[test]
    fn foreground_scan_reenters_transient_decode_under_bound() {
        let temp = tempfile::tempdir().unwrap();
        let audio_path = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            b"speech",
        );
        let jsonl_path = audio_path.with_extension("jsonl");
        let header = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "failed",
                "handler": "transcribe",
                "reason_code": "decode_transient",
                "attempts": 1,
            }
        });
        std::fs::write(&jsonl_path, format!("{header}\n")).unwrap();

        let held = HashSet::new();
        let decision = decide_input(temp.path(), &audio_path, &held);
        assert!(matches!(decision, InputDecision::Process(_)));
        assert!(!held.contains(&audio_path));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn fresh_accepted_audio_enters_without_an_existing_output() {
        let temp = tempfile::tempdir().unwrap();
        let audio = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            b"speech",
        );
        assert_eq!(
            decide_input(temp.path(), &audio, &HashSet::new()),
            InputDecision::Process(None)
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn ordinary_retry_permission_reenters_a_process_held_input_once() {
        use solstone_core_system::work_retry::{self, raw};
        let temp = tempfile::tempdir().unwrap();
        let audio = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            b"speech",
        );
        let output = audio.with_extension("jsonl");
        let header = json!({"raw":"audio.wav","_solstone_processing":{
            "schema":"solstone.processing.v1","state":"failed","handler":"transcribe",
            "reason_code":"local_endpoint_unreachable","attempts":3
        }});
        std::fs::write(&output, format!("{header}\n")).unwrap();
        let held = HashSet::from([audio.clone()]);
        assert_eq!(
            decide_input(temp.path(), &audio, &held),
            InputDecision::Skip
        );
        let evidence = raw::inspect_output(temp.path(), &output).unwrap().unwrap();
        raw::observe(temp.path(), &evidence).unwrap();
        work_retry::request(temp.path(), &evidence.unit).unwrap();
        let InputDecision::Process(Some(context)) = decide_input(temp.path(), &audio, &held) else {
            panic!("ordinary owner permission did not reenter held input");
        };
        raw::consume_context(temp.path(), &output, "transcribe", &context)
            .unwrap()
            .unwrap();
        assert_eq!(
            decide_input(temp.path(), &audio, &held),
            InputDecision::Skip
        );
    }

    #[cfg(all(feature = "full-tests", unix))]
    #[test]
    fn actual_worker_recovers_day_discovery_error_with_its_valid_binding() {
        use std::os::unix::fs::PermissionsExt;
        fn wait(worker: &AudioWorker, predicate: impl Fn(&Value) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let status = worker.status();
                if predicate(&status) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker did not recover discovery: {status}"
                );
                std::thread::park_timeout(Duration::from_millis(20));
            }
        }
        struct RestoreMode(PathBuf);
        impl Drop for RestoreMode {
            fn drop(&mut self) {
                std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let day = temp.path().join("chronicle/20261012");
        std::fs::create_dir_all(&day).unwrap();
        let binding = solstone_core_journal_io::JournalRoot::open(temp.path()).unwrap();
        let _restore = RestoreMode(day.clone());
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o0)).unwrap();
        let worker = AudioWorker::start(
            temp.path().to_owned(),
            temp.path().join("models"),
            temp.path().join("model"),
            temp.path().join("scratch"),
        )
        .unwrap();
        wait(&worker, |status| status["day_error"].is_string());
        binding.revalidate_canonical_binding().unwrap();
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o700)).unwrap();
        wait(&worker, |status| {
            status["day_error"].is_null() && status["state"] == "ready"
        });
        binding.revalidate_canonical_binding().unwrap();
        assert_eq!(worker.status()["generation"], 0);
        drop(worker);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn actual_outer_worker_retains_its_binding_and_recovers_without_resume() {
        fn wait(worker: &AudioWorker, predicate: impl Fn(&Value) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let status = worker.status();
                if predicate(&status) {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker did not reach expected state: {status}"
                );
                std::thread::park_timeout(Duration::from_millis(20));
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("journal");
        std::fs::create_dir(&root).unwrap();
        let worker = AudioWorker::start(
            root.clone(),
            temp.path().join("models"),
            temp.path().join("model"),
            temp.path().join("scratch"),
        )
        .unwrap();
        wait(&worker, |status| status["state"] == "ready");
        std::fs::rename(&root, temp.path().join("original")).unwrap();
        std::fs::create_dir(&root).unwrap();
        let replacement = solstone_core_journal_io::JournalRoot::open(&root).unwrap();
        wait(&worker, |status| status["state"] == "failed");
        std::thread::park_timeout(Duration::from_millis(2200));
        assert_eq!(worker.status()["state"], "failed");
        assert!(!worker.state.cancel.load(Ordering::Acquire));
        drop(replacement);
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::rename(temp.path().join("original"), &root).unwrap();
        wait(&worker, |status| status["state"] == "ready");
        assert_eq!(worker.status()["generation"], 0);
        drop(worker);
    }

    #[test]
    fn foreground_scan_skips_analyzed_and_empty_proofs() {
        let temp = tempfile::tempdir().unwrap();
        let audio_path = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            b"speech",
        );
        let jsonl_path = audio_path.with_extension("jsonl");

        // Analyzed proof
        let header_analyzed = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "handler": "transcribe",
            }
        });
        std::fs::write(&jsonl_path, format!("{header_analyzed}\n")).unwrap();
        let held = HashSet::new();
        let decision = decide_input(temp.path(), &audio_path, &held);
        assert_eq!(decision, InputDecision::Skip);
        assert!(!held.contains(&audio_path));

        // Empty proof
        let header_empty = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "empty",
                "handler": "transcribe",
            }
        });
        std::fs::write(&jsonl_path, format!("{header_empty}\n")).unwrap();
        let decision = decide_input(temp.path(), &audio_path, &held);
        assert_eq!(decision, InputDecision::Skip);
        assert!(!held.contains(&audio_path));
    }

    #[test]
    fn foreground_scan_skips_exhausted_failures() {
        let temp = tempfile::tempdir().unwrap();
        let audio_path = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            b"speech",
        );
        let jsonl_path = audio_path.with_extension("jsonl");
        let held = HashSet::new();

        // Exhausted by attempts bound (3)
        let header_exhausted = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "failed",
                "handler": "transcribe",
                "reason_code": "decode_transient",
                "attempts": 3,
            }
        });
        std::fs::write(&jsonl_path, format!("{header_exhausted}\n")).unwrap();
        let decision = decide_input(temp.path(), &audio_path, &held);
        assert_eq!(decision, InputDecision::Skip);
        assert!(!held.contains(&audio_path));

        // Exhausted by corrupt_input
        let header_corrupt = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "failed",
                "handler": "transcribe",
                "reason_code": "corrupt_input",
                "attempts": 1,
            }
        });
        std::fs::write(&jsonl_path, format!("{header_corrupt}\n")).unwrap();
        let decision = decide_input(temp.path(), &audio_path, &held);
        assert_eq!(decision, InputDecision::Skip);
        assert!(!held.contains(&audio_path));

        // Exhausted by no_audio_stream
        let header_nostream = json!({
            "raw": "audio.wav",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "failed",
                "handler": "transcribe",
                "reason_code": "no_audio_stream",
                "attempts": 1,
            }
        });
        std::fs::write(&jsonl_path, format!("{header_nostream}\n")).unwrap();
        let decision = decide_input(temp.path(), &audio_path, &held);
        assert_eq!(decision, InputDecision::Skip);
        assert!(!held.contains(&audio_path));
    }

    #[test]
    fn foreground_scan_leaves_budget_status_to_the_processing_owner() {
        let temp = tempfile::tempdir().unwrap();
        let segment = temp.path().join("chronicle/20261012/device_audio/120000_1");
        std::fs::create_dir_all(&segment).unwrap();
        let audio_path = segment.join("audio.wav");
        let file = File::create(&audio_path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();

        let state = State {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            external_cancelled: Arc::new(|| false),
            status: Mutex::new(json!({"state":"ready"})),
            recent_failures: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        };
        let mut held = HashSet::new();

        // Missing custody stays pending. The oversize control must actually
        // carry an accepted receipt, or it only exercises that earlier gate.
        assert_eq!(
            decide_input(temp.path(), &audio_path, &held),
            InputDecision::Skip
        );
        setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            &vec![0; 16 * 1024 * 1024 + 1],
        );
        assert_eq!(
            decide_input(temp.path(), &audio_path, &held),
            InputDecision::Process(None)
        );
        // The wrapper still holds genuine custody errors before processing.
        std::fs::write(segment.join("events.jsonl"), b"{broken custody receipt\n").unwrap();
        let error_msg = match decide_input(temp.path(), &audio_path, &held) {
            InputDecision::Hold(error) => error,
            other => panic!("expected custody hold, got {other:?}"),
        };
        state.input_failed(0, &audio_path, &error_msg);
        held.insert(audio_path.clone());
        *state.status.lock().unwrap() = json!({
            "state": "failed",
            "input": audio_path,
            "error": error_msg,
            "failed_inputs": held.len(),
        });

        assert!(held.contains(&audio_path));
        let snapshot = state.snapshot();
        assert_eq!(snapshot["state"], "failed");
        assert_eq!(snapshot["recent_failures"].as_array().unwrap().len(), 1);

        // Later scan in same generation skips held input
        let next = decide_input(temp.path(), &audio_path, &held);
        assert_eq!(next, InputDecision::Skip);

        // Budget error from process_embedded
        let audio_path2 = segment.join("audio2.wav");
        std::fs::write(&audio_path2, b"speech2").unwrap();
        let budget_err = TranscribeError::ParakeetCppDeferred {
            reason: "embedded-duration-budget".into(),
            detail: "budget exceeded".into(),
        };
        held.insert(audio_path2.clone());
        state.input_failed(0, &audio_path2, &budget_err.to_string());
        assert!(held.contains(&audio_path2));
        assert_eq!(
            decide_input(temp.path(), &audio_path2, &held),
            InputDecision::Skip
        );
    }

    #[cfg(feature = "processing")]
    fn host_worker(root: &Path) -> AudioWorker {
        AudioWorker::for_host(
            Arc::new(solstone_core_journal_io::JournalRoot::open(root).unwrap()),
            root.join("unused-models"),
            root.join("unused-model"),
            root.join("scratch"),
            Arc::new(|| false),
        )
    }

    #[cfg(feature = "processing")]
    #[test]
    fn active_host_persists_oversize_refusal_without_repeating_it_on_resume() {
        let temp = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let original = vec![0; 16 * 1024 * 1024 + 1];
        let input = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            &original,
        );
        let worker = host_worker(temp.path());

        assert!(worker.process_host_input(&input, None, &|| false).unwrap());
        let status = worker.status();
        assert_eq!(status["state"], "ready");
        assert!(status["recent_failures"].as_array().unwrap().is_empty());
        let record = solstone_core_processing_record::read_processing_record_header(
            &input.with_extension("jsonl"),
        )
        .unwrap();
        let reason = record["reason_code"].as_str().expect("decode reason");
        assert!(
            reason == solstone_core_processing_record::vocab::REASON_CORRUPT_INPUT
                || reason == solstone_core_processing_record::vocab::REASON_NO_AUDIO_STREAM,
            "{reason}"
        );
        assert_ne!(reason, "embedded-input-budget");
        assert!(solstone_core_processing_record::is_failure_exhausted(
            &record
        ));
        assert!(!input.with_extension("npz").exists());
        assert_eq!(std::fs::read(&input).unwrap(), original);

        assert!(!worker.process_host_input(&input, None, &|| false).unwrap());
        assert!(
            worker.status()["recent_failures"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        worker.resume();
        assert!(!worker.process_host_input(&input, None, &|| false).unwrap());
        assert!(
            worker.status()["recent_failures"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(feature = "processing")]
    #[test]
    fn active_host_cancellation_precedes_input_admission() {
        let temp = tempfile::tempdir().unwrap();
        let input = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            &vec![0; 16 * 1024 * 1024 + 1],
        );
        let worker = host_worker(temp.path());
        worker.cancel();
        assert!(!worker.process_host_input(&input, None, &|| false).unwrap());
        assert!(
            worker.status()["recent_failures"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(!input.with_extension("jsonl").exists());
        assert!(!input.with_extension("npz").exists());
    }

    #[cfg(feature = "processing")]
    #[test]
    fn committed_owner_deletion_requires_absence_even_if_accepted_bytes_reappear() {
        let temp = tempfile::tempdir().unwrap();
        let original = b"synthetic owner audio";
        let input = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            original,
        );
        assert!(accepted_input(temp.path(), &input).unwrap());
        assert!(!input_custody_lost(temp.path(), &input, false));
        assert!(input_custody_lost(temp.path(), &input, true));

        std::fs::remove_file(&input).unwrap();
        assert!(input_custody_lost(temp.path(), &input, false));
        assert!(!input_custody_lost(temp.path(), &input, true));

        std::fs::write(&input, original).unwrap();
        assert!(accepted_input(temp.path(), &input).unwrap());
        assert!(!input_custody_lost(temp.path(), &input, false));
        assert!(input_custody_lost(temp.path(), &input, true));

        std::fs::write(&input, b"changed owner audio").unwrap();
        assert!(!accepted_input(temp.path(), &input).unwrap());
        assert!(input_custody_lost(temp.path(), &input, false));
        assert!(input_custody_lost(temp.path(), &input, true));

        #[cfg(unix)]
        {
            std::fs::remove_file(&input).unwrap();
            std::os::unix::fs::symlink(input.with_extension("missing"), &input).unwrap();
            assert!(input_custody_lost(temp.path(), &input, true));
        }
    }

    #[cfg(feature = "processing")]
    #[test]
    fn active_host_cancels_on_custody_loss_after_admission() {
        // These mutations occur at a real embedded checkpoint after admission.
        // No model should be reached once the accepted input is lost or cancelled.
        for change in ["cancel", "changed", "missing", "oversize", "receipt"] {
            let temp = tempfile::tempdir().unwrap();
            let input = setup_fixture_audio(
                temp.path(),
                "20261012",
                "device_audio",
                "120000_1",
                "audio.wav",
                b"speech",
            );
            let worker = host_worker(temp.path());
            let changed = Cell::new(false);
            let checkpoint = || {
                if worker.status()["state"] == "processing" && !changed.replace(true) {
                    match change {
                        "changed" => std::fs::write(&input, b"change").unwrap(),
                        "missing" => std::fs::remove_file(&input).unwrap(),
                        "oversize" => File::options()
                            .write(true)
                            .open(&input)
                            .unwrap()
                            .set_len(16 * 1024 * 1024 + 1)
                            .unwrap(),
                        "receipt" => {
                            std::fs::write(
                                input.parent().unwrap().join("events.jsonl"),
                                b"broken\n",
                            )
                            .unwrap();
                        }
                        "cancel" => (),
                        _ => unreachable!(),
                    }
                }
                change == "cancel" && changed.get()
            };
            assert!(
                !worker
                    .process_host_input(&input, None, &checkpoint)
                    .unwrap()
            );
            assert!(
                changed.get(),
                "{change} did not reach the embedded checkpoint"
            );
            assert_eq!(worker.status()["state"], "cancelled", "{change}");
            assert!(
                worker.status()["recent_failures"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(!input.with_extension("jsonl").exists(), "{change}");
            assert!(!input.with_extension("npz").exists(), "{change}");
        }
    }

    #[test]
    fn foreground_scan_day_discovery_error_does_not_abort_other_days() {
        let temp = tempfile::tempdir().unwrap();
        let valid_audio = setup_fixture_audio(
            temp.path(),
            "20261012",
            "device_audio",
            "120000_1",
            "audio.wav",
            b"speech",
        );
        let bad_day = temp.path().join("chronicle/20261011");
        std::fs::create_dir_all(&bad_day).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bad_day, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        let mut day_errors = Vec::new();
        let inputs = discover_audio_files(temp.path(), &mut |_| {}, &mut |day, error| {
            day_errors.push(format!("{}: {error}", day.display()));
        })
        .collect::<Vec<_>>();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bad_day, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert!(!day_errors.is_empty());
        assert_eq!(inputs, vec![valid_audio]);

        let state = State {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            external_cancelled: Arc::new(|| false),
            status: Mutex::new(json!({"state":"ready"})),
            recent_failures: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        };
        let err_str = day_errors.join("; ");
        let mut status = state.status.lock().unwrap();
        status["day_error"] = json!(err_str.clone());
        status["error"] = json!(err_str);
        drop(status);

        let snap = state.snapshot();
        assert_eq!(snap["day_error"], json!(err_str));
        assert_eq!(snap["state"], "ready");
    }

    #[test]
    fn foreground_scan_binding_failure_aborts_run() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("journal");
        std::fs::create_dir(&root).unwrap();
        let authority = solstone_core_journal_io::JournalRoot::open(&root).unwrap();
        std::fs::rename(&root, temp.path().join("away")).unwrap();
        let state = State {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            external_cancelled: Arc::new(|| false),
            status: Mutex::new(json!({"state":"ready"})),
            recent_failures: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        };
        assert!(
            run(
                &state,
                &authority,
                Path::new("unused"),
                Path::new("unused"),
                Path::new("unused"),
            )
            .is_err()
        );
    }

    #[test]
    fn resume_cannot_erase_cancellation_of_an_active_inference() {
        let state = State {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            external_cancelled: Arc::new(|| false),
            status: Mutex::new(json!({})),
            recent_failures: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
        };
        let active = state.generation.load(Ordering::Acquire);
        assert!(!state.cancelled(active));
        state.request_cancel();
        let worker = AudioWorker {
            state: Arc::new(state),
            thread: None,
            #[cfg(feature = "processing")]
            host_config: None,
        };
        worker.resume(); // resume before inference returns
        let state = &worker.state;
        assert!(state.cancelled(active));
        assert!(!state.cancelled(state.generation.load(Ordering::Acquire)));
        // Per-input failures leave cancel false. Resume must still retire that
        // attempt, causing the scan's failure cache to clear before retry.
        let failed_attempt = state.generation.load(Ordering::Acquire);
        worker.resume();
        assert!(state.cancelled(failed_attempt));
        assert!(!state.cancelled(state.generation.load(Ordering::Acquire)));
    }

    #[test]
    fn input_failure_survives_headline_replacement_and_resume_identifies_old_attempt() {
        let worker = AudioWorker {
            state: Arc::new(State {
                stop: AtomicBool::new(false),
                cancel: AtomicBool::new(false),
                generation: AtomicU64::new(7),
                external_cancelled: Arc::new(|| false),
                status: Mutex::new(json!({"state":"processing"})),
                recent_failures: Mutex::new(VecDeque::new()),
                wake: Condvar::new(),
            }),
            thread: None,
            #[cfg(feature = "processing")]
            host_config: None,
        };
        worker.state.input_failed(
            7,
            Path::new("target/audio.wav"),
            "size differs from pinned model",
        );
        worker.state.input_failed(
            7,
            Path::new("older/audio.m4a"),
            "duration exceeds 30 seconds",
        );
        *worker.state.status.lock().unwrap() = json!({"state":"failed","input":"older/audio.m4a","error":"duration exceeds 30 seconds"});
        let snapshot = worker.status();
        assert_eq!(snapshot["input"], "older/audio.m4a");
        assert_eq!(snapshot["generation"], 7);
        assert_eq!(snapshot["recent_failures"][0]["input"], "target/audio.wav");
        assert_eq!(
            snapshot["recent_failures"][0]["error"],
            "size differs from pinned model"
        );
        worker.resume();
        let resumed = worker.status();
        assert_eq!(resumed["generation"], 8);
        assert_eq!(resumed["recent_failures"][0]["generation"], 7);
        for index in 0..70 {
            worker.state.input_failed(
                8,
                Path::new(&format!("input-{index}/audio.wav")),
                "fixture failure",
            );
        }
        let bounded = worker.status();
        let history = bounded["recent_failures"].as_array().unwrap();
        assert_eq!(history.len(), 64);
        assert_eq!(history[0]["input"], "input-6/audio.wav");
        assert_eq!(history[63]["input"], "input-69/audio.wav");
        assert!(history.iter().all(|row| row["generation"] == 8));
    }

    #[test]
    fn only_accepted_identical_input_is_admitted_and_oversize_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let segment = temp.path().join("chronicle/20261012/device_audio/120000_3");
        std::fs::create_dir_all(&segment).unwrap();
        let input = segment.join("audio.wav");
        std::fs::write(&input, b"speech").unwrap();
        assert!(!accepted_input(temp.path(), &input).unwrap());
        let mut event = DeviceIngestEvent {
            record_type: "device_ingest".into(),
            record_version: 1,
            outcome: "partial".into(),
            protocol_version: 3,
            cid: "fixture".into(),
            source: "audio".into(),
            stream: "device_audio".into(),
            day: "20261012".into(),
            segment: "120000_3".into(),
            meta: Default::default(),
            extra: Default::default(),
            files: vec![FileDescriptor {
                submitted: "audio.wav".into(),
                written: "audio.wav".into(),
                size: 6,
                sha256: format!("{:x}", Sha256::digest(b"speech")),
                extra: Default::default(),
            }],
        };
        append_durable_event(&segment, &DurableEvent::DeviceIngest(event.clone())).unwrap();
        assert!(!accepted_input(temp.path(), &input).unwrap());
        event.outcome = "accepted".into();
        append_durable_event(&segment, &DurableEvent::DeviceIngest(event)).unwrap();
        assert!(accepted_input(temp.path(), &input).unwrap());
        std::fs::write(&input, b"changed").unwrap();
        assert!(!accepted_input(temp.path(), &input).unwrap());
        File::options()
            .write(true)
            .open(&input)
            .unwrap()
            .set_len(16 * 1024 * 1024 + 1)
            .unwrap();
        assert!(!accepted_input(temp.path(), &input).unwrap());
    }
}
