// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_indexer_query::{OwnerBoundary, SearchRequest};
use solstone_core_journal_io as jio;
use std::ffi::{CStr, CString, OsStr, c_char};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

#[cfg(any(target_os = "android", all(test, feature = "full-tests")))]
mod callosum_endpoint;
#[cfg(unix)]
mod owner_response;
mod retirement;

static CANCEL: AtomicBool = AtomicBool::new(false);
struct ServingInstance {
    handle: Option<journal_mobile_serving::MobileServing>,
    retirement: Option<retirement::Retirement>,
    root: std::sync::Arc<jio::JournalRoot>,
    host: Option<journal_mobile_serving::foreground::ForegroundProcessingHost>,
}

struct RetiringServing {
    handle: journal_mobile_serving::MobileServing,
    host: Option<journal_mobile_serving::foreground::ForegroundProcessingHost>,
}

impl ServingInstance {
    fn begin_retirement(&mut self) -> Result<(), String> {
        if self.retirement.is_some() {
            return Ok(());
        }
        let handle = self
            .handle
            .take()
            .ok_or("server stop failed; journal retained")?;
        let owner = RetiringServing {
            handle,
            host: self.host.take(),
        };
        match retirement::Retirement::start(owner, std::sync::Arc::clone(&self.root), |owner| {
            if let Some(host) = &owner.host {
                host.shutdown().map_err(|_| ())?;
            }
            owner.handle.shutdown_for_retirement().map_err(|_| ())
        }) {
            Ok(retirement) => {
                self.retirement = Some(retirement);
                Ok(())
            }
            Err(owner) => {
                self.handle = Some(owner.handle);
                self.host = owner.host;
                Err("could not start server retirement; journal retained".into())
            }
        }
    }
    fn handle(&self) -> Result<&journal_mobile_serving::MobileServing, String> {
        self.handle
            .as_ref()
            .ok_or_else(|| "server stop pending".into())
    }
    fn status(&self) -> Result<Value, String> {
        let mut status = self.handle()?.status();
        status["admission_generation"] = json!(THINKING_GENERATION.load(Ordering::SeqCst));
        status["interface_address"] = json!(self.handle()?.interface_address());
        if let Some(host) = self.host.as_ref() {
            status["audio"] = host.audio_status();
            if host.failed() {
                status["failed"] = json!(true);
            }
        }
        Ok(status)
    }
}

static SERVING: std::sync::Mutex<Option<ServingInstance>> = std::sync::Mutex::new(None);
static SERVING_STOP: std::sync::Mutex<Option<journal_mobile_serving::MobileServingStop>> =
    std::sync::Mutex::new(None);

static PROCESSING_STOP: std::sync::Mutex<Option<std::sync::Arc<AtomicBool>>> =
    std::sync::Mutex::new(None);

fn request_serving_stop() {
    close_thinking_admission(&THINKING_GENERATION);
    let processing_stop = PROCESSING_STOP
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(stop) = processing_stop {
        stop.store(true, Ordering::Release);
    }
    let stop = SERVING_STOP
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    if let Some(stop) = stop {
        stop.request_stop();
    }
}

static GENERATE: std::sync::LazyLock<solstone_core_generate_wire::EndpointRuntime> =
    std::sync::LazyLock::new(solstone_core_generate_wire::EndpointRuntime::default);
static THINKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
// The low bit closes admission. Only a successful start in the same generation
// can reopen it; diagnostic `begin` only resets the separate probe flag.
static THINKING_GENERATION: AtomicU64 = AtomicU64::new(1);

fn close_thinking_admission(state: &AtomicU64) {
    let _ = state.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |generation| {
        Some(generation.wrapping_add(2) | 1)
    });
}

fn open_thinking_admission(state: &AtomicU64, generation: u64) -> bool {
    state
        .compare_exchange(
            generation,
            generation & !1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}

fn admitted_generation(command: &Value, state: &AtomicU64) -> Result<u64, String> {
    let generation = command["admission_generation"]
        .as_u64()
        .ok_or("thinking admission generation required")?;
    if state.load(Ordering::SeqCst) != generation {
        return Err("thinking admission changed".into());
    }
    Ok(generation)
}

fn thinking_cancelled(state: &AtomicU64, generation: u64, root: &jio::JournalRoot) -> bool {
    state.load(Ordering::SeqCst) != generation || root.revalidate_canonical_binding().is_err()
}

fn serving(root: &Path, command: &str) -> Result<Value, String> {
    let command: Value = serde_json::from_str(command).map_err(|e| e.to_string())?;
    let operation = command["op"].as_str().ok_or("serving op required")?;
    if operation == "stop" {
        request_serving_stop();
    }
    // Capture before any wait, so a request cannot adopt a later lifetime.
    let generation = if matches!(operation, "generate" | "talent" | "start" | "audio-enable") {
        admitted_generation(&command, &THINKING_GENERATION)?
    } else {
        THINKING_GENERATION.load(Ordering::SeqCst)
    };
    if matches!(operation, "generate" | "talent" | "audio-enable") && generation & 1 != 0 {
        return Err("thinking is stopped".into());
    }
    // Stop drains the bounded provider operation before retiring its journal.
    // Keep owner status and network controls independent of this wait.
    let _thinking = if matches!(operation, "generate" | "talent" | "stop") {
        Some(THINKING.lock().map_err(|_| "thinking lock poisoned")?)
    } else {
        None
    };
    if matches!(operation, "generate" | "talent") {
        if THINKING_GENERATION.load(Ordering::SeqCst) != generation {
            return Err("thinking is stopped".into());
        }
        // Thinking runs independently of serving controls and never retains the
        // serving mutex across a provider call.
        let binding = {
            let server = SERVING.lock().map_err(|_| "serving lock poisoned")?;
            let current = server.as_ref().ok_or("server stopped")?;
            current.handle()?;
            current
                .root
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            if fs::canonicalize(root).map_err(|e| e.to_string())? != current.root.canonical_path() {
                return Err("thinking journal does not match the serving journal".into());
            }
            std::sync::Arc::clone(&current.root)
        };
        let cancelled = || thinking_cancelled(&THINKING_GENERATION, generation, &binding);
        let cancellation = solstone_core_generate::Cancellation::from_fn(&cancelled);
        let root = binding.canonical_path();
        let raw = command["payload"]
            .as_str()
            .ok_or("thinking request required")?;
        let generate = |request: &solstone_core_generate::GenerateRequest| -> Result<
            solstone_core_generate::GenerateResponse,
            solstone_core_generate::ClientError,
        > {
            binding
                .revalidate_canonical_binding()
                .map_err(|e| solstone_core_generate::ClientError::Decode(e.to_string()))?;
            let response = solstone_core_generate_wire::respond(
                request,
                &GENERATE,
                root,
                |_| None,
                &cancellation,
            )
            .map_err(solstone_core_generate::ClientError::Decode)?;
            binding
                .revalidate_canonical_binding()
                .map_err(|e| solstone_core_generate::ClientError::Decode(e.to_string()))?;
            Ok(response)
        };
        if operation == "generate" {
            let request = solstone_core_generate_wire::parse_one_shot_request(raw)?;
            let response = generate(&request).map_err(|e| e.to_string())?;
            if matches!(
                response,
                solstone_core_generate::GenerateResponse::Cancelled
            ) {
                return Err("generate cancelled".into());
            }
            return serde_json::from_str(&solstone_core_generate::encode_one_shot_response(
                &response,
            )?)
            .map_err(|e| e.to_string());
        }
        let request = serde_json::from_str::<serde_json::Map<String, Value>>(raw)
            .map_err(|e| e.to_string())?;
        let payload = root
            .parent()
            .ok_or("app directory missing")?
            .join("journal-payload");
        let paths = solstone_core_talent_runtime::prepare::RuntimePaths {
            talent_root: payload.join("solstone/talent"),
            apps_root: payload.join("solstone/apps"),
            templates_dir: payload.join("solstone/think/templates"),
        };
        let context = solstone_core_talent_runtime::ExecutionContext {
            journal: root.to_owned(),
        };
        let mut events = Vec::new();
        let outcome = solstone_core_talent_runtime::execute_request(
            request,
            &paths,
            &context,
            &generate,
            &mut events,
            &cancellation,
        );
        return Ok(json!({"outcome":format!("{outcome:?}"),
            "events":String::from_utf8(events).map_err(|e|e.to_string())?}));
    }
    let mut server = SERVING.lock().map_err(|_| "serving lock poisoned")?;
    match operation {
        "start" => {
            match retirement::admission() {
                retirement::Outcome::Stopped => {}
                retirement::Outcome::Pending => return Err("server stop pending".into()),
                retirement::Outcome::Failed => {
                    return Err("server stop failed; journal retained".into());
                }
            }
            if let Some(current) = server.as_mut()
                && let Some(retirement) = current.retirement.as_mut()
            {
                match retirement.wait(std::time::Duration::ZERO) {
                    retirement::Outcome::Stopped => *server = None,
                    retirement::Outcome::Pending => return Err("server stop pending".into()),
                    retirement::Outcome::Failed => {
                        return Err("server stop failed; journal retained".into());
                    }
                }
            }
            // Rebind observes the same live owner; it cannot reopen a stopping lifetime.
            admitted_generation(&command, &THINKING_GENERATION)?;
            if let Some(current) = server.as_ref() {
                if current.host.as_ref().is_some_and(|host| host.failed()) {
                    return Err("server stop failed; journal retained".into());
                }
                let handle = current.handle()?;
                if generation & 1 != 0 || handle.stop_handle().is_stopping() {
                    return Err("server stop pending".into());
                }
                current
                    .root
                    .revalidate_canonical_binding()
                    .map_err(|e| e.to_string())?;
                if fs::canonicalize(root).map_err(|e| e.to_string())?
                    != current.root.canonical_path()
                {
                    return Err("thinking journal does not match the serving journal".into());
                }
                let status = current.status()?;
                if command["address"]
                    .as_str()
                    .and_then(|address| address.parse().ok())
                    != Some(handle.interface_address())
                {
                    return Err("server already started".into());
                }
                return Ok(status);
            }
            let address = command["address"]
                .as_str()
                .ok_or("interface address required")?
                .parse()
                .map_err(|e: std::net::AddrParseError| e.to_string())?;
            // Android loads the packaged runtime from its read-only native library directory.
            let library = command["onnx_library"]
                .as_str()
                .ok_or("ONNX runtime library required")?;
            ort::init_from(library).map_err(|e| e.to_string())?.commit();
            let binding =
                std::sync::Arc::new(jio::JournalRoot::open(root).map_err(|e| e.to_string())?);
            #[cfg(target_os = "android")]
            callosum_endpoint::prepare(binding.canonical_path())?;
            let payload_dir = root
                .parent()
                .ok_or("app directory missing")?
                .join("journal-payload");
            let handle =
                match journal_mobile_serving::MobileServing::start_with_payload_for_retirement_observed(
                    root.to_path_buf(),
                    address,
                    root.parent()
                        .ok_or("app directory missing")?
                        .join("journal-payload"),
                    std::sync::Arc::new(move || {
                        THINKING_GENERATION.load(Ordering::SeqCst) & !1 != generation & !1
                    }),
                    std::sync::Arc::new(move |stop| {
                        *SERVING_STOP.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(stop.clone());
                        if THINKING_GENERATION.load(Ordering::SeqCst) & !1 != generation & !1 {
                            stop.request_stop();
                        }
                    }),
                ) {
                    Ok(handle) => handle,
                    Err(failure) => {
                        let (message, owner) = failure.into_parts();
                        if let Some(owner) = owner {
                            *SERVING_STOP.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(owner.stop_handle());
                            let mut current = ServingInstance {
                                handle: Some(*owner),
                                retirement: None,
                                root: binding,
                                host: None,
                            };
                            let _ = current.begin_retirement();
                            *server = Some(current);
                        }
                        return Err(message);
                    }
                };
            *SERVING_STOP.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle.stop_handle());
            let processing_stop = std::sync::Arc::new(AtomicBool::new(false));
            *PROCESSING_STOP.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(std::sync::Arc::clone(&processing_stop));
            let backup = handle.backup_dependencies();
            let mut host = match journal_mobile_serving::foreground::ForegroundProcessingHost::open_with_cancellation(
                std::sync::Arc::clone(&binding), &payload_dir,
                std::sync::Arc::new(move || THINKING_GENERATION.load(Ordering::SeqCst) & !1 != generation & !1),
                processing_stop,
                backup,
            ) {
                Ok(host) => host,
                Err(error) => {
                    let mut current = ServingInstance {
                        handle: Some(handle), retirement: None, root: binding, host: None,
                    };
                    let _ = current.begin_retirement();
                    *server = Some(current);
                    return Err(error);
                }
            };
            if let Err(error) = host.start_workers() {
                let mut current = ServingInstance {
                    handle: Some(handle),
                    retirement: None,
                    root: binding,
                    host: Some(host),
                };
                let _ = current.begin_retirement();
                *server = Some(current);
                return Err(error);
            }

            *server = Some(ServingInstance {
                handle: Some(handle),
                retirement: None,
                root: binding,
                host: Some(host),
            });
            if !open_thinking_admission(&THINKING_GENERATION, generation) {
                let current = server.as_mut().unwrap();
                current.handle()?.stop_handle().request_stop();
                current.begin_retirement()?;
                return Err("stop requested during startup; journal retained".into());
            }
            server.as_ref().unwrap().status()
        }
        "stop" => {
            if let Some(current) = server.as_mut() {
                current.begin_retirement()?;
                match current
                    .retirement
                    .as_mut()
                    .unwrap()
                    .wait(std::time::Duration::ZERO)
                {
                    retirement::Outcome::Stopped => {}
                    retirement::Outcome::Pending => {}
                    retirement::Outcome::Failed => {
                        return Err("server stop failed; journal retained".into());
                    }
                }
            }
            match retirement::wait_for_process(std::time::Duration::from_secs(5)) {
                retirement::Outcome::Stopped => {}
                retirement::Outcome::Pending => {
                    return Err("server stop pending; owned work still active".into());
                }
                retirement::Outcome::Failed => {
                    return Err("server stop failed; journal retained".into());
                }
            }
            *SERVING_STOP.lock().unwrap_or_else(|p| p.into_inner()) = None;
            *server = None;
            GENERATE.drain_idle_pool();
            Ok(json!({"stopped":true}))
        }
        "status" => {
            let Some(current) = server.as_ref() else {
                return match retirement::inspect() {
                    retirement::Outcome::Stopped => Err("server stopped".into()),
                    retirement::Outcome::Pending => Ok(json!({"stopping":true,"failed":false})),
                    retirement::Outcome::Failed => Ok(json!({"stopping":true,"failed":true})),
                };
            };
            if current.retirement.is_some() {
                let (stopping, failed) = match retirement::inspect() {
                    retirement::Outcome::Pending => (true, false),
                    retirement::Outcome::Stopped => (false, false),
                    retirement::Outcome::Failed => (true, true),
                };
                Ok(json!({
                    "stopping": stopping,
                    "failed": failed,
                    "root_current": current.root.revalidate_canonical_binding().is_ok(),
                }))
            } else if current.handle()?.stop_handle().is_stopping() {
                let mut status = current.status()?;
                let failed = status["failed"].as_bool().unwrap_or(false)
                    || retirement::inspect() == retirement::Outcome::Failed;
                status["stopping"] = json!(true);
                status["failed"] = json!(failed);
                status["root_current"] = json!(current.root.revalidate_canonical_binding().is_ok());
                Ok(status)
            } else {
                current.status()
            }
        }
        "index" => server.as_ref().ok_or("server stopped")?.handle()?.index(),
        "service-tools" => {
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let restic = solstone_core_backup_runtime::resolve::resolve_restic_member(&executable)
                .map(|p| p.display().to_string())
                .map_err(|e| e.to_string());
            let nvattest = solstone_core_spp_ratls::resolve_nvattest_from_current_exe()
                .map(|p| p.binary.display().to_string())
                .map_err(|e| format!("{}:{}", e.failure.reason_code, e.detail));
            Ok(
                json!({"executable":executable,"temporary_directory":std::env::temp_dir(),"restic":format!("{restic:?}"),
                "nvattest":format!("{nvattest:?}"),
                "confidential_offered":solstone_core_spp_ratls::confidential_verifier_on_this_platform()}),
            )
        }
        "audio-enable" => {
            // Repeat after SERVING acquisition; Stop may have arrived during that wait.
            admitted_generation(&command, &THINKING_GENERATION)?;
            if generation & 1 != 0 {
                return Err("thinking is stopped".into());
            }
            let current = server.as_ref().ok_or("server stopped")?;
            current
                .root
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            if fs::canonicalize(root).map_err(|e| e.to_string())? != current.root.canonical_path() {
                return Err("thinking journal does not match the serving journal".into());
            }
            let files = root.parent().ok_or("app files missing")?;
            let host = current
                .host
                .as_ref()
                .ok_or("foreground processing stopped")?;
            host.enable_audio(
                files.join("audio-models"),
                files.join("audio-models/tdt-0.6b-v3-q8_0.gguf"),
                files.join("audio-scratch"),
            )?;
            current.status()
        }
        "audio-cancel" => {
            let current = server.as_ref().ok_or("server stopped")?;
            let host = current
                .host
                .as_ref()
                .ok_or("foreground processing stopped")?;
            host.cancel_audio();
            Ok(json!({"cancel_requested": true}))
        }
        "audio-resume" => {
            let current = server.as_ref().ok_or("server stopped")?;
            let host = current
                .host
                .as_ref()
                .ok_or("foreground processing stopped")?;
            host.resume_audio();
            Ok(json!({"resumed": true}))
        }
        "owner" => {
            let response = server
                .as_ref()
                .ok_or("server stopped")?
                .handle()?
                .request_owner(
                    command["method"].as_str().ok_or("method required")?,
                    command["uri"].as_str().ok_or("uri required")?,
                    command["body"].as_str().unwrap_or(""),
                )?;
            Ok(json!({"status":response.status,"body":response.body}))
        }
        _ => Err("unknown serving operation".into()),
    }
}

fn seed_mixed(root: &Path, turn: usize) -> Result<(), Box<dyn std::error::Error>> {
    // Synthetic fixture generation belongs only to this development harness.
    // Production activity writes continue through the facets authority.
    let directory = root.join("facets/work/activities");
    fs::create_dir_all(&directory)?;
    let authority = jio::JournalRoot::open(&directory)?;
    let days: Vec<usize> = if turn == 0 {
        (1..=16).collect()
    } else {
        vec![turn % 16 + 1]
    };
    for day in days {
        let mut bytes = Vec::new();
        for row in 0..256 {
            let record = json!({"id":format!("mixed-{day}-{row}"),"title":format!("Synthetic day {day} row {row}"),
                "story":{"body":format!("androidsentinel day{day} uniqueentry{row} revision{turn}")}});
            writeln!(&mut bytes, "{record}")?;
        }
        let name = format!("202609{day:02}.jsonl");
        let result = jio::atomic_replace_bound(&authority, OsStr::new(&name), &bytes, 0o600)?;
        if !matches!(result, jio::BoundAtomicOutcome::Published { .. }) {
            return Err(format!("fixture publication unconfirmed: {result:?}").into());
        }
    }
    Ok(())
}

fn publication_race(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    use std::sync::{Arc, Barrier};
    let dir = root.join(format!("publication-race-{}", std::process::id()));
    fs::create_dir(&dir)?;
    let barrier = Arc::new(Barrier::new(17));
    let finished = Arc::new(AtomicBool::new(false));
    let observer_dir = dir.clone();
    let observer_barrier = barrier.clone();
    let observer_finished = finished.clone();
    let observer = std::thread::spawn(move || -> Result<usize, String> {
        observer_barrier.wait();
        let start = Instant::now();
        let mut observations = 0;
        while start.elapsed().as_secs() < 5 {
            match fs::read(observer_dir.join("winner")) {
                Ok(bytes) => {
                    if bytes.len() != 256 * 1024
                        || bytes[0] >= 16
                        || !bytes.iter().all(|v| *v == bytes[0])
                    {
                        return Err("reader observed partial publication".into());
                    }
                    observations += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            if observer_finished.load(Ordering::Acquire) && observations > 0 {
                return Ok(observations);
            }
            std::thread::yield_now();
        }
        Err("publication observer timed out".into())
    });
    let mut threads = Vec::new();
    for i in 0..16_u8 {
        let dir = dir.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let bytes = vec![i; 256 * 1024];
            barrier.wait();
            let authority = jio::JournalRoot::open(&dir).map_err(|e| e.to_string())?;
            match jio::write_bytes_exclusive_bound_detailed(
                &authority,
                OsStr::new("winner"),
                &bytes,
                0o600,
            ) {
                Ok(result) if result.is_fully_confirmed() => Ok(true),
                Ok(result) => Err(format!("publication unconfirmed: {result:?}")),
                Err(error) if error.source.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                Err(error) => Err(error.to_string()),
            }
        }));
    }
    let results: Vec<_> = threads.into_iter().map(|thread| thread.join()).collect();
    finished.store(true, Ordering::Release);
    let observations = observer.join().map_err(|_| "observer panicked")??;
    let mut winners = 0;
    for result in results {
        winners += usize::from(result.map_err(|_| "publication thread panicked")??);
    }
    let bytes = fs::read(dir.join("winner"))?;
    if winners != 1 || bytes.len() != 256 * 1024 || !bytes.iter().all(|v| *v == bytes[0]) {
        return Err("concurrent publication violated single-complete-winner".into());
    }
    Ok(
        json!({"contenders":16,"winners":winners,"bytes":bytes.len(),"winner_byte":bytes[0],"complete_reader_observations":observations}),
    )
}

fn publication_faults(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let authority = jio::JournalRoot::open(root)?;
    let mut checked = Vec::new();
    for (i, primitive) in [
        jio::BoundPublicationPrimitive::FileSync,
        jio::BoundPublicationPrimitive::ExclusivePublish,
        jio::BoundPublicationPrimitive::ParentSync,
        jio::BoundPublicationPrimitive::CompletionObserve,
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("fault-{}-{i}", std::process::id());
        let (result, consumed) =
            jio::run_with_bound_publication_fault(primitive, 1, libc::EIO, || {
                jio::write_bytes_exclusive_bound_detailed(
                    &authority,
                    OsStr::new(&name),
                    b"complete fault fixture",
                    0o600,
                )
            });
        if !consumed {
            return Err("publication fault not consumed".into());
        }
        if i < 2 {
            if result.is_ok() || root.join(&name).exists() {
                return Err("pre-publication failure exposed destination".into());
            }
        } else {
            let outcome = result?;
            if outcome.is_fully_confirmed()
                || !matches!(outcome.cleanup, jio::StageCleanup::Removed)
                || fs::read(root.join(&name))? != b"complete fault fixture"
            {
                return Err("publication failure incorrectly confirmed".into());
            }
            if i == 2 && !matches!(outcome.durability, jio::MetadataDurability::Unproven { .. }) {
                return Err("directory sync failure lost uncertainty".into());
            }
            if i == 3
                && !matches!(
                    outcome.final_name,
                    jio::FinalNameConfirmation::Unverified { .. }
                )
            {
                return Err("final observation failure lost uncertainty".into());
            }
        }
        checked.push(format!("{primitive:?}"));
    }
    Ok(json!({"passed":true,"checked":checked}))
}

fn work(root: &Path, scan: bool) -> Result<Value, Box<dyn std::error::Error>> {
    let begin = Instant::now();
    let authority = jio::JournalRoot::open(root)?;
    authority.revalidate_canonical_binding()?;
    let atomic = jio::atomic_replace_bound(
        &authority,
        OsStr::new("checkpoint.json"),
        b"{\"committed\":true}",
        0o600,
    )?;
    if !matches!(atomic, jio::BoundAtomicOutcome::Published { .. }) {
        return Err(format!("checkpoint publication unconfirmed: {atomic:?}").into());
    }
    let scan_start = Instant::now();
    let report = if scan {
        format!(
            "{:?}",
            solstone_core_indexer_store::scan::scan_journal(root, false)?
        )
    } else {
        "not_requested".into()
    };
    let scan_ms = scan_start.elapsed().as_secs_f64() * 1000.;
    let request = SearchRequest {
        query: "androidsentinel".into(),
        limit: 8,
        counts: true,
        ..Default::default()
    };
    let query_start = Instant::now();
    let result = solstone_core_indexer_query::search(
        root,
        OwnerBoundary,
        &request,
        chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
    )?;
    let query_ms = query_start.elapsed().as_secs_f64() * 1000.;
    let db = solstone_core_indexer_store::db::open_index_reader(root)?;
    let integrity: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    let mode: String = db.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    Ok(
        json!({"scan":report,"scan_ms":scan_ms,"query_ms":query_ms,"total_ms":begin.elapsed().as_secs_f64()*1000.,"result":result,"sqlite_check":integrity,"journal_mode":mode,"atomic":format!("{atomic:?}"),"sqlite_version":rusqlite::version(),"proc_status":fs::read_to_string("/proc/self/status")?.lines().filter(|l| l.starts_with("VmRSS:")||l.starts_with("VmHWM:")||l.starts_with("Threads:")).collect::<Vec<_>>()}),
    )
}

fn resources(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    fn size(dir: &Path) -> std::io::Result<u64> {
        let mut bytes = 0;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            bytes += if meta.is_dir() {
                size(&entry.path())?
            } else {
                meta.len()
            };
        }
        Ok(bytes)
    }
    Ok(
        json!({"smaps_rollup":fs::read_to_string("/proc/self/smaps_rollup")?,
        "fd_count":fs::read_dir("/proc/self/fd")?.count(),"journal_bytes":size(root)?,
        "status":fs::read_to_string("/proc/self/status")?}),
    )
}

fn run(root: &Path, op: &str, count: usize) -> Result<Value, Box<dyn std::error::Error>> {
    if op == "thinking-generation" {
        return Ok(json!({"generation":THINKING_GENERATION.load(Ordering::SeqCst)}));
    }
    if op == "identity" {
        return Ok(
            json!({"pid":std::process::id(),"uid":unsafe{libc::getuid()},"selinux":fs::read_to_string("/proc/self/attr/current")?,"maps":fs::read_to_string("/proc/self/maps")?.lines().filter(|l|l.contains("journal_runtime")||l.contains("journalbridge")).collect::<Vec<_>>(),"sqlite_version":rusqlite::version()}),
        );
    }
    if op == "crash" {
        std::process::abort();
    }
    if op == "cancel" {
        CANCEL.store(true, Ordering::Release);
        request_serving_stop();
        // Observe only after cancellation is signalled. Never delay Stop to
        // obtain evidence; null means busy, absent or not observed, not idle.
        let native_call_active_after_cancel = SERVING
            .try_lock()
            .ok()
            .and_then(|server| server.as_ref()?.host.as_ref()?.native_call_active());
        return Ok(json!({"cancel_requested":true,
            "native_call_active_after_cancel":native_call_active_after_cancel}));
    }
    if op == "begin" {
        CANCEL.store(false, Ordering::Release);
        return Ok(json!({"cancel_reset":true}));
    }
    if let Some(command) = op.strip_prefix("serve:") {
        return serving(root, command).map_err(Into::into);
    }
    fs::create_dir_all(root)?;
    match op {
        "resources" => resources(root),
        "authority" => {
            let authority = jio::JournalRoot::open(root)?;
            let id = authority.identity();
            let reopen = jio::JournalRoot::open(root)?;
            if id != reopen.identity() {
                return Err("root identity changed".into());
            }
            let probe = root.join(format!("binding-{}", std::process::id()));
            let moved = root.join(format!("binding-moved-{}", std::process::id()));
            fs::create_dir(&probe)?;
            let bound = jio::JournalRoot::open(&probe)?;
            jio::write_bytes_exclusive_bound(&bound, OsStr::new("record"), b"bound inode", 0o600)?;
            fs::rename(&probe, &moved)?;
            fs::create_dir(&probe)?;
            let replacement_refused = bound.revalidate_canonical_binding().is_err();
            let original_retained = jio::read_bytes_bound(&bound, OsStr::new("record"))?.as_deref()
                == Some(b"bound inode".as_slice());
            let link = root.join("symlink-negative");
            if !link.exists() {
                std::os::unix::fs::symlink("checkpoint.json", &link)?;
            }
            let symlink_refused =
                jio::read_bytes_bound(&authority, OsStr::new("symlink-negative")).is_err();
            let overwrite_refused = jio::write_bytes_exclusive_bound(
                &authority,
                OsStr::new("checkpoint.json"),
                b"must not overwrite",
                0o600,
            )
            .is_err();
            let target = root.join("authority-lock");
            let lock = jio::hold_lock(&target, jio::LockOptions::default())?;
            let held = jio::lock_is_held(&target)?;
            drop(lock);
            let released = !jio::lock_is_held(&target)?;
            Ok(
                json!({"reopen_identity":true,"root_replacement_refused":replacement_refused,"bound_original_retained":original_retained,"symlink_refused":symlink_refused,"overwrite_refused":overwrite_refused,"lock_held":held,"lock_released":released}),
            )
        }
        "seed-mixed" => {
            seed_mixed(root, 0)?;
            work(root, true)
        }
        "mutate" => {
            seed_mixed(root, count)?;
            work(root, true)
        }
        "publication-race" => publication_race(root),
        "publication-faults" => publication_faults(root),
        "publication" => {
            let authority = jio::JournalRoot::open(root)?;
            let name = format!("exclusive-{}-{count}.txt", std::process::id());
            let result = jio::write_bytes_exclusive_bound_detailed(
                &authority,
                OsStr::new(&name),
                b"complete synthetic publication",
                0o600,
            )?;
            if !result.is_fully_confirmed() {
                return Err(format!("publication unconfirmed: {result:?}").into());
            }
            let refused = jio::write_bytes_exclusive_bound_detailed(
                &authority,
                OsStr::new(&name),
                b"replacement",
                0o600,
            )
            .unwrap_err();
            if refused.source.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(refused.into());
            }
            Ok(
                json!({"publication":format!("{result:?}"),"confirmed":true,"existing_refused":true}),
            )
        }
        "work" => work(root, true),
        "query" => work(root, false),
        "repeat" => {
            let start = Instant::now();
            let mut elapsed = Vec::new();
            let mut last = Value::Null;
            for _ in 0..count.min(100_000) {
                if CANCEL.load(Ordering::Acquire) {
                    break;
                }
                last = work(root, true)?;
                elapsed.push(last["total_ms"].as_f64().unwrap());
            }
            Ok(
                json!({"iterations":elapsed.len(),"cancelled":CANCEL.load(Ordering::Acquire),"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"iteration_ms":elapsed,"last":last}),
            )
        }
        "atomic-crash" => {
            let authority = jio::JournalRoot::open(root)?;
            let marker_path = root.join("atomic-barrier.json");
            let _ = jio::run_with_bound_publication_barrier(
                jio::BoundPublicationPrimitive::Rename,
                1,
                move || {
                    let mut marker = fs::File::create(marker_path).unwrap();
                    marker
                        .write_all(b"before rename, staged file synced")
                        .unwrap();
                    marker.sync_all().unwrap();
                    // Uncatchable process death at the real publication barrier.
                    unsafe {
                        libc::kill(libc::getpid(), libc::SIGKILL);
                    }
                },
                || {
                    jio::atomic_replace_bound(
                        &authority,
                        OsStr::new("checkpoint.json"),
                        b"interrupted replacement",
                        0o600,
                    )
                },
            );
            Err("atomic crash barrier unexpectedly returned".into())
        }
        "exclusive-crash-before" | "exclusive-crash-after" => {
            let authority = jio::JournalRoot::open(root)?;
            let after = op.ends_with("after");
            let name = if after {
                "exclusive-after"
            } else {
                "exclusive-before"
            };
            // A unique fixture seam on each full run, never owner data.
            if root.join(name).exists() {
                fs::remove_file(root.join(name))?;
            }
            let primitive = if after {
                jio::BoundPublicationPrimitive::PostPublishObserve
            } else {
                jio::BoundPublicationPrimitive::ExclusivePublish
            };
            let _ = jio::run_with_bound_publication_barrier(
                primitive,
                1,
                || unsafe {
                    libc::kill(libc::getpid(), libc::SIGKILL);
                },
                || {
                    jio::write_bytes_exclusive_bound_detailed(
                        &authority,
                        OsStr::new(name),
                        b"complete exclusive crash fixture",
                        0o600,
                    )
                },
            );
            Err("exclusive crash barrier unexpectedly returned".into())
        }
        "recovery" => {
            let authority = jio::JournalRoot::open(root)?;
            let checkpoint = jio::read_bytes_bound(&authority, OsStr::new("checkpoint.json"))?;
            let db = solstone_core_indexer_store::db::open_index_reader(root)?;
            let ghosts: i64 = db.query_row(
                "SELECT count(*) FROM chunks WHERE content='uncommittedghost'",
                [],
                |r| r.get(0),
            )?;
            Ok(
                json!({"checkpoint":String::from_utf8(checkpoint.ok_or("checkpoint missing")?)?,"uncommitted_ghost_rows":ghosts}),
            )
        }
        "transaction-crash" => {
            // Deliberate failure seam in the existing real SQLite index, not a new job ledger.
            let db = rusqlite::Connection::open(solstone_core_indexer_store::db::db_path(root))?;
            db.execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;")?;
            let changed = db.execute(
                "UPDATE chunks SET content=?1 WHERE rowid=(SELECT min(rowid) FROM chunks)",
                ["uncommittedghost"],
            )?;
            let mut marker = fs::File::create(root.join("transaction-open.json"))?;
            marker.write_all(
                json!({"changed":changed,"pid":std::process::id()})
                    .to_string()
                    .as_bytes(),
            )?;
            marker.sync_all()?;
            unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
            }
            Err("transaction crash unexpectedly returned".into())
        }
        "snapshot" => {
            let db = rusqlite::Connection::open(solstone_core_indexer_store::db::db_path(root))?;
            db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(json!({"checkpointed":true}))
        }
        _ => Err("unknown operation".into()),
    }
}

/// Startup-only app metadata and signed APK service assets. No owner command
/// or environment setting reaches this function.
///
/// # Safety
/// Directory pointers must be live NUL-terminated strings. Asset pointers must
/// remain readable for their stated lengths. Release the result with `journal_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_admit_service_tools(
    native: *const c_char,
    resources: *const c_char,
    manifest: *const u8,
    manifest_len: usize,
    receipt: *const u8,
    receipt_len: usize,
) -> *mut c_char {
    let output = std::panic::catch_unwind(|| -> Result<Value, String> {
        const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
        if native.is_null()
            || resources.is_null()
            || manifest.is_null()
            || receipt.is_null()
            || manifest_len == 0
            || receipt_len == 0
            || manifest_len > MAX_ASSET_BYTES
            || receipt_len > MAX_ASSET_BYTES
        {
            return Err("app metadata and bounded service assets required".into());
        }
        let native = unsafe { CStr::from_ptr(native) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let resources = unsafe { CStr::from_ptr(resources) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let manifest = unsafe { std::slice::from_raw_parts(manifest, manifest_len) };
        let receipt = unsafe { std::slice::from_raw_parts(receipt, receipt_len) };
        solstone_core_spp_attest::android_service_tools::install_android_service_tools(
            Path::new(native),
            Path::new(resources),
            manifest,
            receipt,
        )
        .map_err(|e| format!("{}:{e}", e.code()))?;
        Ok(json!({"installed":true}))
    });
    let value = match output {
        Ok(Ok(value)) => json!({"ok":true,"value":value}),
        Ok(Err(error)) => json!({"ok":false,"error":error}),
        Err(_) => json!({"ok":false,"error":"native service-tool admission panicked"}),
    };
    CString::new(value.to_string()).unwrap().into_raw()
}

/// # Safety
/// `root` and `op` must point to live NUL-terminated strings for this call.
/// Release the returned owned string exactly once with `journal_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_run(
    root: *const c_char,
    op: *const c_char,
    count: usize,
) -> *mut c_char {
    let output = std::panic::catch_unwind(|| {
        let root = unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let op = unsafe { CStr::from_ptr(op) }
            .to_str()
            .map_err(|e| e.to_string())?;
        run(Path::new(root), op, count).map_err(|e| e.to_string())
    });
    let value = match output {
        Ok(Ok(v)) => json!({"ok":true,"value":v}),
        Ok(Err(e)) => json!({"ok":false,"error":e}),
        Err(panic) => {
            json!({"ok":false,"error":format!("native panic: {}", panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied()).unwrap_or("unknown payload"))})
        }
    };
    CString::new(value.to_string()).unwrap().into_raw()
}

/// # Safety
/// `value` must be null or an unreleased pointer returned by a journal FFI call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod thinking_admission_tests {
    use super::*;

    #[test]
    fn original_root_binding_cancels_a_replacement_without_poisoning_restored_binding() {
        struct Temporary(std::path::PathBuf);
        impl Drop for Temporary {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temporary(std::env::temp_dir().join(format!(
            "journal-thinking-root-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        )));
        fs::create_dir(&temp.0).unwrap();
        let root = temp.0.join("journal");
        fs::create_dir(&root).unwrap();
        let binding = jio::JournalRoot::open(&root).unwrap();
        let state = AtomicU64::new(0);
        assert!(!thinking_cancelled(&state, 0, &binding));
        fs::rename(&root, temp.0.join("original")).unwrap();
        fs::create_dir(&root).unwrap();
        let replacement = jio::JournalRoot::open(&root).unwrap();
        assert!(thinking_cancelled(&state, 0, &binding));
        drop(replacement);
        fs::remove_dir_all(&root).unwrap();
        fs::rename(temp.0.join("original"), &root).unwrap();
        assert!(!thinking_cancelled(&state, 0, &binding));
        close_thinking_admission(&state);
        assert!(thinking_cancelled(&state, 0, &binding));
    }

    #[test]
    fn stop_between_java_ticket_check_and_native_capture_keeps_admission_closed() {
        let state = AtomicU64::new(1);
        assert!(open_thinking_admission(&state, 1));
        let admitted = state.load(Ordering::SeqCst);
        let old_command = json!({"admission_generation":admitted});
        assert_eq!(admitted_generation(&old_command, &state).unwrap(), admitted);
        assert_eq!(admitted & 1, 0); // Java ticket has passed its check.
        close_thinking_admission(&state); // Stop arrives before Native.run.
        let native_capture = state.load(Ordering::SeqCst);
        assert_ne!(native_capture & 1, 0); // An old request cannot adopt this epoch.
        assert_ne!(native_capture, admitted); // In-flight requests also cancel.
        assert!(admitted_generation(&old_command, &state).is_err());
        assert!(!open_thinking_admission(&state, admitted)); // Stale start cannot reopen.
        assert!(open_thinking_admission(&state, native_capture));
        let replacement = state.load(Ordering::SeqCst);
        assert_eq!(replacement & 1, 0);
        assert_ne!(replacement, admitted);
        assert!(admitted_generation(&old_command, &state).is_err());
        let start = state.load(Ordering::SeqCst);
        close_thinking_admission(&state);
        assert!(!open_thinking_admission(&state, start)); // Stop during startup wins.
        assert_ne!(state.load(Ordering::SeqCst) & 1, 0);
    }
}

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod retirement_start_tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    struct HeldTls {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl Drop for HeldTls {
        fn drop(&mut self) {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(30)).unwrap();
        }
    }
    thread_local! { static TLS: RefCell<Option<HeldTls>> = const { RefCell::new(None) }; }
    struct Release(Option<mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(release) = self.0.take() {
                let _ = release.send(());
            }
        }
    }

    #[test]
    fn native_worker_creation_failure_returns_original_owner() {
        use std::os::unix::process::CommandExt;
        let selector = format!(
            "{}::limited_retirement_child",
            module_path!().split_once("::").unwrap().1
        );
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &selector, "--ignored", "--test-threads=1"])
            .env("SOLSTONE_RETIREMENT_SPAWN_FAILURE_CHILD", "1");
        // RLIMIT_NPROC does not restrict UID0. Only this synthetic child drops UID.
        if unsafe { libc::geteuid() } == 0 {
            command.gid(65534).uid(65534);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("1 passed; 0 failed")
        );
    }

    #[test]
    #[ignore = "isolated ordinary child controls its own thread-creation limit"]
    fn limited_retirement_child() {
        use std::sync::atomic::{AtomicBool, Ordering};
        assert_eq!(
            std::env::var("SOLSTONE_RETIREMENT_SPAWN_FAILURE_CHILD").unwrap(),
            "1"
        );
        assert_ne!(unsafe { libc::geteuid() }, 0);
        let root = Arc::new(());
        // Initialize and prove the actual executor before denying new OS threads.
        let mut positive = retirement::Retirement::start((), root.clone(), |_| Ok(()))
            .ok()
            .unwrap();
        assert_eq!(
            positive.wait(Duration::from_secs(5)),
            retirement::Outcome::Stopped
        );
        drop(positive);
        struct Owner(Arc<AtomicBool>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let destroyed = Arc::new(AtomicBool::new(false));
        let called = Arc::new(AtomicBool::new(false));
        let call = called.clone();
        let mut original = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NPROC, &mut original) },
            0
        );
        let limited = libc::rlimit {
            rlim_cur: 0,
            rlim_max: original.rlim_max,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &limited) }, 0);
        let result = retirement::Retirement::start(Owner(destroyed.clone()), root, move |_| {
            call.store(true, Ordering::Release);
            Ok(())
        });
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &original) }, 0);
        let returned = result
            .err()
            .expect("real native thread creation must refuse under RLIMIT_NPROC0");
        assert!(Arc::ptr_eq(&returned.0, &destroyed));
        assert!(!destroyed.load(Ordering::Acquire));
        assert!(!called.load(Ordering::Acquire));
        assert_eq!(retirement::admission(), retirement::Outcome::Stopped);
        drop(returned);
        assert!(destroyed.load(Ordering::Acquire));
    }

    #[test]
    fn production_start_requires_real_join_after_local_slot_loss() {
        let directory = std::env::temp_dir().join(format!(
            "android-jni-retirement-start-{}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let first = directory.join("journal");
        let second = directory.join("other-journal");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let payload = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../core/payload")
            .canonicalize()
            .unwrap();
        std::os::unix::fs::symlink(payload, directory.join("journal-payload")).unwrap();
        let binding = Arc::new(jio::JournalRoot::open(&first).unwrap());
        let (entered, observed) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let mut release = Release(Some(release));
        let observer = retirement::Retirement::start((), binding, move |_| {
            TLS.with(|tls| {
                *tls.borrow_mut() = Some(HeldTls {
                    entered,
                    release: held,
                })
            });
            Ok(())
        })
        .ok()
        .unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(observer);
        *SERVING.lock().unwrap() = None;
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = probe.local_addr().unwrap().to_string();
        drop(probe);
        let library = std::env::var("SOLSTONE_JNI_TEST_ONNX_LIBRARY")
            .expect("maintained pinned host ONNX input required");
        let start = || {
            json!({"op":"start","address":address,"onnx_library":library,
            "admission_generation":THINKING_GENERATION.load(Ordering::SeqCst)})
            .to_string()
        };
        for root in [&first, &second] {
            assert_eq!(serving(root, &start()).unwrap_err(), "server stop pending");
            assert!(SERVING.lock().unwrap().is_none());
        }
        assert_eq!(
            serving(&first, "{\"op\":\"status\"}").unwrap()["stopping"],
            true
        );
        let stopped = Instant::now();
        assert_eq!(
            serving(&first, "{\"op\":\"stop\"}").unwrap_err(),
            "server stop pending; owned work still active"
        );
        assert!(stopped.elapsed() < Duration::from_secs(6));
        assert!(SERVING.lock().unwrap().is_none());
        release.0.take().unwrap().send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while retirement::admission() == retirement::Outcome::Pending && Instant::now() < deadline {
            std::thread::park_timeout(Duration::from_millis(10));
        }
        assert_eq!(retirement::admission(), retirement::Outcome::Stopped);
        assert_eq!(
            serving(&first, "{\"op\":\"stop\"}").unwrap()["stopped"],
            true
        );
        assert_eq!(serving(&first, &start()).unwrap()["root_current"], true);
        let active_epoch = THINKING_GENERATION.load(Ordering::SeqCst);
        let live = serving(&first, &start()).unwrap();
        assert_eq!(live["root_current"], true);
        assert_eq!(
            SERVING
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .handle()
                .unwrap()
                .interface_address()
                .to_string(),
            address
        );
        // This fixture has not finalized first-run setup; its device door stays withheld.
        assert_eq!(live["door_address"], Value::Null);
        assert_eq!(THINKING_GENERATION.load(Ordering::SeqCst), active_epoch);
        assert!(
            serving(&second, &start()).is_err(),
            "rebind must not adopt a different root"
        );
        let audio = json!({"op":"audio-enable","admission_generation":active_epoch}).to_string();
        serving(&first, &audio).unwrap();
        let audio_generation = serving(&first, &audio).unwrap()["audio"]["generation"].clone();
        assert_eq!(audio_generation, 0);
        let deadline = Instant::now() + Duration::from_secs(3);
        while serving(&first, r#"{"op":"status"}"#).unwrap()["audio"]["state"] != "ready" {
            assert!(
                Instant::now() < deadline,
                "actual audio worker did not become ready"
            );
            std::thread::park_timeout(Duration::from_millis(10));
        }
        assert_eq!(
            serving(&first, r#"{"op":"status"}"#).unwrap()["audio"]["cancel_requested"],
            false
        );
        // The real early Stop must signal all these owners while thinking is held.
        let held_thinking = THINKING.lock().unwrap();
        let cancellation_at = Instant::now();
        run(&first, "cancel", 1).unwrap();
        assert!(cancellation_at.elapsed() < Duration::from_millis(500));
        assert_eq!(
            serving(&first, r#"{"op":"status"}"#).unwrap()["stopping"],
            true
        );
        assert!(
            serving(&first, &audio).is_err(),
            "queued old audio-enable cannot install or reset work"
        );
        assert_eq!(
            serving(&first, &start()).unwrap_err(),
            "server stop pending"
        );
        {
            let server = SERVING.lock().unwrap();
            let owner = server.as_ref().unwrap().handle().unwrap();
            assert!(owner.stop_handle().is_stopping());
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let audio = serving(&first, r#"{"op":"status"}"#).unwrap()["audio"].clone();
            if audio["cancel_requested"] == true {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "host audio must receive Stop before thinking is released: {audio}"
            );
            std::thread::park_timeout(Duration::from_millis(10));
        }
        drop(held_thinking);

        assert!(SERVING.lock().unwrap().as_ref().unwrap().handle.is_some());
        assert_eq!(
            serving(&first, "{\"op\":\"stop\"}").unwrap()["stopped"],
            true
        );
        assert!(SERVING.lock().unwrap().is_none());
        assert_eq!(retirement::admission(), retirement::Outcome::Stopped);
        let binding = Arc::new(jio::JournalRoot::open(&first).unwrap());
        let original = Arc::downgrade(&binding);
        let mut failed = retirement::Retirement::start((), binding, |_| Err(()))
            .ok()
            .unwrap();
        assert_eq!(
            failed.wait(Duration::from_secs(5)),
            retirement::Outcome::Failed
        );
        drop(failed);
        for root in [&first, &second] {
            assert_eq!(
                serving(root, &start()).unwrap_err(),
                "server stop failed; journal retained"
            );
        }
        assert_eq!(
            serving(&first, "{\"op\":\"stop\"}").unwrap_err(),
            "server stop failed; journal retained"
        );
        assert_eq!(
            serving(&first, "{\"op\":\"status\"}").unwrap()["failed"],
            true
        );
        original
            .upgrade()
            .unwrap()
            .revalidate_canonical_binding()
            .unwrap();
        assert!(SERVING.lock().unwrap().is_none());
        // The process retains this failed owner's root until native process exit.
        fs::remove_dir_all(&directory).unwrap();
    }
}
