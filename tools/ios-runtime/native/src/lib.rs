// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
//! Development-only iOS helper bridge. Fault operations never enter a release binary.
use chrono::NaiveDate;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_indexer_query::{
    OwnerBoundary, OwnerIndex, SearchRequest, open_owner_index, search,
};
use solstone_core_journal_io::{
    JournalRoot,
    atomic::{self, BoundAtomicOutcome, BoundPublicationPrimitive},
};
use std::{
    ffi::{CStr, CString, OsStr, c_char},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex, Once, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static ROOT: Mutex<Option<Arc<JournalRoot>>> = Mutex::new(None);
static QUERY: Mutex<Option<OwnerIndex>> = Mutex::new(None);
struct ServingInstance {
    serving: journal_mobile_serving::MobileServing,
    processing: Option<journal_mobile_serving::foreground::ForegroundProcessingHost>,
}
impl ServingInstance {
    fn status(&self) -> Value {
        let mut status = self.serving.status();
        if let Some(host) = &self.processing {
            #[cfg(feature = "audio")]
            {
                status["audio"] = host.audio_status();
            }
            if host.failed() {
                status["failed"] = json!(true);
            }
        }
        status
    }
    fn drain(&self) -> Result<(), String> {
        // Keep both actual owners in SERVING until both checked drains succeed.
        let processing = self
            .processing
            .as_ref()
            .map(|host| host.shutdown())
            .transpose();
        let serving = self.serving.shutdown_for_retirement();
        if processing.is_err() || serving.is_err() {
            return Err("processing or serving drain failed; journal retained".into());
        }
        Ok(())
    }
}
static SERVING: Mutex<Option<ServingInstance>> = Mutex::new(None);
static SERVING_STOP: Mutex<Option<journal_mobile_serving::MobileServingStop>> = Mutex::new(None);
static PROCESSING_STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
// Low bit closes admission; a queued operation keeps its ingress lifetime.
static PROCESSING_GENERATION: AtomicU64 = AtomicU64::new(1);

fn close_processing_admission(state: &AtomicU64) {
    let _ = state.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |generation| {
        Some(generation.wrapping_add(2) | 1)
    });
}
fn admit_processing(v: &Value, state: &AtomicU64) -> Result<u64, String> {
    let generation = v["admissionGeneration"]
        .as_u64()
        .ok_or("processing admission required")?;
    if state.load(Ordering::SeqCst) != generation {
        return Err("processing admission changed".into());
    }
    Ok(generation)
}
fn open_processing_admission(state: &AtomicU64, generation: u64) -> bool {
    state
        .compare_exchange(
            generation,
            generation & !1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
}
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_processing_generation() -> u64 {
    PROCESSING_GENERATION.load(Ordering::SeqCst)
}
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_processing_request_stop() {
    close_processing_admission(&PROCESSING_GENERATION);
    if let Some(stop) = PROCESSING_STOP
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        stop.store(true, Ordering::Release);
    }
    if let Some(stop) = SERVING_STOP
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
    {
        stop.request_stop();
    }
}
static PAYLOAD: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(target_os = "ios")]
static BACKUP_ADMISSION_DIAGNOSTIC: Mutex<&'static str> = Mutex::new("not_attempted");
static GENERATE: LazyLock<solstone_core_generate_wire::EndpointRuntime> =
    LazyLock::new(solstone_core_generate_wire::EndpointRuntime::default);

#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_backup_admit() -> bool {
    let result = solstone_core_backup_runtime::ios_callable::admit_ios_backup_at_startup();
    if let Ok(mut diagnostic) = BACKUP_ADMISSION_DIAGNOSTIC.lock() {
        *diagnostic = result
            .as_ref()
            .map(|_| "admitted")
            .unwrap_or_else(|reason| reason);
    }
    result.is_ok()
}

#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_backup_admission_diagnostic() -> *mut c_char {
    let diagnostic = BACKUP_ADMISSION_DIAGNOSTIC
        .lock()
        .map(|value| *value)
        .unwrap_or("diagnostic_unavailable");
    CString::new(diagnostic)
        .expect("fixed admission reason has no NUL")
        .into_raw()
}

// The development host has no desktop logger. Retain existing door warnings
// so a dropped TLS carrier can be correlated with its server-side cause.
struct DoorWarnings;
impl log::Log for DoorWarnings {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let target = metadata.target();
        metadata.level() <= log::Level::Warn
            && (target.starts_with("solstone_core_convey_shell::door")
                || target.starts_with("solstone_core_backup")
                || target.starts_with("journal_mobile_serving"))
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "ios-log time_ms={} pid={} {} {}: {}",
                chrono::Utc::now().timestamp_millis(),
                std::process::id(),
                record.target(),
                record.level(),
                record.args()
            );
        }
    }
    fn flush(&self) {}
}
fn install_door_warnings() {
    static LOGGER: DoorWarnings = DoorWarnings;
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Warn);
        }
    });
}
fn fixture(i: u64, revision: u64) -> String {
    format!(
        "# Synthetic memory {i}\n\nauroraruntime document {i}: copper kite revision {revision}.\n"
    )
}
fn query_limit(root: &Path, limit: usize) -> Result<Value, String> {
    query_with(root, limit, true)
}
fn query_with(root: &Path, limit: usize, reuse: bool) -> Result<Value, String> {
    let request = SearchRequest {
        query: "auroraruntime".into(),
        limit,
        ..Default::default()
    };
    let date = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
    let response = if reuse {
        let mut guard = QUERY.lock().map_err(|e| e.to_string())?;
        if guard.is_none() {
            *guard = Some(open_owner_index(root, OwnerBoundary).map_err(|e| e.to_string())?);
        }
        guard.as_mut().unwrap().search(&request, date)
    } else {
        search(root, OwnerBoundary, &request, date)
    }
    .map_err(|e| e.to_string())?;
    serde_json::to_value(response).map_err(|e| e.to_string())
}
fn query(root: &Path) -> Result<Value, String> {
    query_limit(root, 4096)
}
fn replace(root: &JournalRoot, name: &str, data: &[u8]) -> Result<(), String> {
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    match atomic::atomic_replace_bound(root, OsStr::new(name), data, 0o600)
        .map_err(|e| e.to_string())?
    {
        BoundAtomicOutcome::Published { .. } => Ok(()),
        other => Err(format!("publication not fully confirmed: {other:?}")),
    }
}
fn create(root: &JournalRoot, name: &str, data: &[u8]) -> Result<(), String> {
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    let result = atomic::write_bytes_exclusive_bound_detailed(root, OsStr::new(name), data, 0o600)
        .map_err(|e| e.to_string())?;
    if result.is_fully_confirmed() {
        Ok(())
    } else {
        Err(format!("publication not fully confirmed: {result:?}"))
    }
}
fn inventory(root: &Path) -> Result<Value, String> {
    let files=std::fs::read_dir(root.join("chronicle/20261012/talents")).map_err(|e|e.to_string())?.map(|e|{
        let e=e.map_err(|e|e.to_string())?;
        Ok(json!({"name":e.file_name().to_string_lossy(),"text":std::fs::read_to_string(e.path()).map_err(|e|e.to_string())?}))
    }).collect::<Result<Vec<_>,String>>()?;
    let entries = std::fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .map(|e| {
            e.map(|e| e.file_name().to_string_lossy().into_owned())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut sizes = serde_json::Map::new();
    for name in [
        "indexer/journal.sqlite",
        "indexer/journal.sqlite-wal",
        "indexer/journal.sqlite-shm",
    ] {
        sizes.insert(
            name.into(),
            json!(
                std::fs::metadata(root.join(name))
                    .map(|m| m.len())
                    .unwrap_or(0)
            ),
        );
    }
    Ok(
        json!({"files":files,"entries":entries,"sizes":sizes,"checkpoint":std::fs::read_to_string(root.join("checkpoint.bin")).ok(),"committed":std::fs::read_to_string(root.join("committed.bin")).ok(),"exclusive":std::fs::read_to_string(root.join("exclusive.bin")).ok()}),
    )
}
fn wifi_address() -> Result<std::net::Ipv4Addr, String> {
    let mut interfaces: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut cursor = interfaces;
    let mut address = None;
    while !cursor.is_null() {
        let interface = unsafe { &*cursor };
        if !interface.ifa_addr.is_null()
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
            && unsafe { CStr::from_ptr(interface.ifa_name) }.to_bytes() == b"en0"
        {
            let socket = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_in) };
            let ip = std::net::Ipv4Addr::from(socket.sin_addr.s_addr.to_ne_bytes());
            if ip.is_private() {
                address = Some(ip);
            }
        }
        cursor = interface.ifa_next;
    }
    unsafe { libc::freeifaddrs(interfaces) };
    let ip = address.ok_or("no private en0 IPv4 address")?;
    Ok(ip)
}
fn run(v: Value) -> Result<Value, String> {
    let op = v["op"].as_str().ok_or("missing operation")?;
    if op == "serve-stop" {
        journal_ios_processing_request_stop();
    }
    let generation = if matches!(
        op,
        "serve-start" | "serve-generate" | "serve-talent" | "audio-enable" | "audio-resume"
    ) {
        let generation = admit_processing(&v, &PROCESSING_GENERATION)?;
        if op != "serve-start" && generation & 1 != 0 {
            return Err("processing is stopped".into());
        }
        Some(generation)
    } else {
        None
    };
    if op == "serve-crash" {
        std::process::abort();
    }
    if op == "activity" {
        return Ok(json!({"active": ACTIVE.load(Ordering::SeqCst)}));
    }
    if op == "cancel" {
        CANCEL.store(true, Ordering::SeqCst);
        return Ok(json!({"cancel_requested":true}));
    }
    let mut guard = ROOT.lock().map_err(|e| e.to_string())?;
    if op == "open" {
        let requested = PathBuf::from(v["root"].as_str().ok_or("missing helper-owned root")?);
        if let Some(current) = guard.as_ref() {
            current
                .revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            if current.canonical_path()
                != std::fs::canonicalize(&requested).map_err(|e| e.to_string())?
            {
                return Err("worker already bound to another journal".into());
            }
        } else {
            std::fs::create_dir_all(&requested).map_err(|e| e.to_string())?;
            *guard = Some(Arc::new(
                JournalRoot::open(&requested).map_err(|e| e.to_string())?,
            ));
        }
    }
    let root = guard.as_ref().ok_or("open the journal first")?;
    root.revalidate_canonical_binding()
        .map_err(|e| e.to_string())?;
    let p = root.canonical_path();
    match op {
        "serve-start" => {
            let generation = generation.unwrap();
            admit_processing(&v, &PROCESSING_GENERATION)?;
            let mut serving = SERVING.lock().map_err(|e| e.to_string())?;
            if serving.is_some() {
                return Err("serving already started".into());
            }
            let port = v["count"]
                .as_u64()
                .filter(|p| (1024..=65535).contains(p))
                .ok_or("port must be between 1024 and 65535")? as u16;
            let address = std::net::SocketAddrV4::new(wifi_address()?, port);
            let payload = PathBuf::from(v["payload"].as_str().ok_or("missing bundled payload")?);
            #[cfg(target_os = "ios")]
            {
                let target = std::env::temp_dir().join("callosum.sock");
                if !target.is_absolute() || target.as_os_str().len() >= 104 {
                    return Err("app-private Callosum target exceeds Unix socket path bound".into());
                }
                let alias = p.join("health/callosum.sock");
                std::fs::create_dir_all(alias.parent().ok_or("missing health directory")?)
                    .map_err(|e| e.to_string())?;
                match std::fs::symlink_metadata(&alias) {
                    Ok(metadata)
                        if metadata.file_type().is_symlink()
                            && std::fs::read_link(&alias).map_err(|e| e.to_string())? == target => {
                    }
                    // iOS changes the app container on reinstall. This alias is
                    // app-owned transient IPC, so replace its link (never its
                    // old target) with the current app-private endpoint.
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let temporary = alias.with_file_name(format!(
                            ".callosum-alias-{}-{}",
                            std::process::id(),
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_err(|e| e.to_string())?
                                .as_nanos()
                        ));
                        std::os::unix::fs::symlink(&target, &temporary)
                            .map_err(|e| e.to_string())?;
                        if let Err(error) = std::fs::rename(&temporary, &alias) {
                            let _ = std::fs::remove_file(temporary);
                            return Err(error.to_string());
                        }
                    }
                    Ok(_) => return Err("Callosum endpoint already has another binding".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        std::os::unix::fs::symlink(&target, &alias).map_err(|e| e.to_string())?
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            let runtime = match journal_mobile_serving::MobileServing::start_with_payload_for_retirement_observed(
                p.to_owned(),
                address,
                payload.clone(),
                Arc::new(move || PROCESSING_GENERATION.load(Ordering::SeqCst) & !1 != generation & !1),
                Arc::new(move |stop| {
                    *SERVING_STOP.lock().unwrap_or_else(|p| p.into_inner()) = Some(stop.clone());
                    if PROCESSING_GENERATION.load(Ordering::SeqCst) & !1 != generation & !1 {
                        stop.request_stop();
                    }
                }),
            ) {
                Ok(runtime) => runtime,
                Err(failure) => {
                    let (message, owner) = failure.into_parts();
                    *serving = owner.map(|owner| ServingInstance { serving: *owner, processing: None });
                    journal_ios_processing_request_stop();
                    return Err(message);
                }
            };
            let processing_stop = Arc::new(AtomicBool::new(false));
            *PROCESSING_STOP.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(Arc::clone(&processing_stop));
            let backup = runtime.backup_dependencies();
            *serving = Some(ServingInstance {
                serving: runtime,
                processing: None,
            });
            let host = journal_mobile_serving::foreground::ForegroundProcessingHost::open_with_cancellation(
                Arc::clone(root), &payload,
                Arc::new(move || PROCESSING_GENERATION.load(Ordering::SeqCst) & !1 != generation & !1),
                processing_stop, backup,
            );
            let host = match host {
                Ok(host) => host,
                Err(error) => {
                    journal_ios_processing_request_stop();
                    return Err(error);
                }
            };
            let current = serving.as_mut().unwrap();
            current.processing = Some(host);
            if let Err(error) = current.processing.as_mut().unwrap().start_workers() {
                journal_ios_processing_request_stop();
                return Err(error);
            }
            if !open_processing_admission(&PROCESSING_GENERATION, generation) {
                journal_ios_processing_request_stop();
                return Err("stop requested during startup; journal retained".into());
            }
            *PAYLOAD.lock().map_err(|e| e.to_string())? = Some(payload);
            let status = current.status();
            Ok(json!({"address":address.to_string(),"status":status}))
        }
        #[cfg(feature = "audio")]
        "audio-enable" => {
            let models = PathBuf::from(
                v["payload"]
                    .as_str()
                    .ok_or("missing bundled audio models")?,
            );
            let scratch = p.join(".audio-scratch");
            let model = match v["mode"].as_str().unwrap_or("reopen") {
                "missing-model" => scratch.join("missing.gguf"),
                "corrupt-model" => {
                    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
                    let model = scratch.join("corrupt.gguf");
                    std::fs::write(&model, b"synthetic corrupt model")
                        .map_err(|e| e.to_string())?;
                    model
                }
                _ => models.join("tdt-0.6b-v3-q8_0.gguf"),
            };
            let mut serving = SERVING.lock().map_err(|e| e.to_string())?;
            admit_processing(&v, &PROCESSING_GENERATION)?;
            let current = serving.as_mut().ok_or("serving is stopped")?;
            let host = current.processing.as_ref().ok_or("processing is stopped")?;
            host.enable_audio(models.clone(), model, scratch)?;
            Ok(current.status())
        }
        #[cfg(feature = "audio")]
        "audio-cancel" | "audio-resume" => {
            let serving = SERVING.lock().map_err(|e| e.to_string())?;
            let serving = serving.as_ref().ok_or("serving is stopped")?;
            let host = serving.processing.as_ref().ok_or("processing is stopped")?;
            if op == "audio-cancel" {
                host.cancel_audio();
            } else {
                admit_processing(&v, &PROCESSING_GENERATION)?;
                host.resume_audio();
            }
            Ok(serving.status())
        }
        "serve-stop" => {
            let mut serving = SERVING.lock().map_err(|e| e.to_string())?;
            if let Some(current) = serving.as_ref() {
                current.drain()?;
            }
            serving.take();
            *SERVING_STOP.lock().unwrap_or_else(|p| p.into_inner()) = None;
            *PROCESSING_STOP.lock().unwrap_or_else(|p| p.into_inner()) = None;
            GENERATE.drain_idle_pool();
            PAYLOAD.lock().map_err(|e| e.to_string())?.take();
            Ok(json!({"stopped":true}))
        }
        "serve-generate" | "serve-talent" => {
            let payload = PAYLOAD
                .lock()
                .map_err(|e| e.to_string())?
                .clone()
                .ok_or("serving is stopped")?;
            let raw = v["payload"].as_str().ok_or("missing thinking request")?;
            let generation = generation.unwrap();
            let cancelled = || {
                PROCESSING_GENERATION.load(Ordering::SeqCst) != generation
                    || root.revalidate_canonical_binding().is_err()
            };
            if cancelled() {
                return Err("processing is stopped".into());
            }
            let cancellation = solstone_core_generate::Cancellation::from_fn(&cancelled);
            let generate = |request: &solstone_core_generate::GenerateRequest| {
                solstone_core_generate_wire::respond(request, &GENERATE, p, |_| None, &cancellation)
                    .map_err(solstone_core_generate::ClientError::Decode)
            };
            if op == "serve-generate" {
                let request = solstone_core_generate_wire::parse_one_shot_request(raw)?;
                let response = generate(&request).map_err(|e| e.to_string())?;
                let encoded = solstone_core_generate::encode_one_shot_response(&response)?;
                return serde_json::from_str(&encoded).map_err(|e| e.to_string());
            }
            let request = serde_json::from_str::<serde_json::Map<String, Value>>(raw)
                .map_err(|e| e.to_string())?;
            let paths = solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: payload.join("solstone/talent"),
                apps_root: payload.join("solstone/apps"),
                templates_dir: payload.join("solstone/think/templates"),
            };
            let context = solstone_core_talent_runtime::ExecutionContext {
                journal: p.to_owned(),
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
            Ok(json!({"outcome":format!("{outcome:?}"),
                "events":String::from_utf8(events).map_err(|e|e.to_string())?}))
        }
        "serve-index" => {
            let report = solstone_core_indexer_store::scan::scan_journal(p, false)
                .map_err(|e| e.to_string())?;
            if report.failed != 0 {
                return Err(format!("index scan failed: {:?}", report.failures));
            }
            Ok(
                json!({"indexed":report.indexed,"removed":report.removed,"skipped":report.skipped,
                "failed":report.failed,"warnings":report.warnings}),
            )
        }
        #[cfg(unix)]
        "serve-rotation-fault-arm" => {
            if SERVING.lock().map_err(|e| e.to_string())?.is_none() {
                return Err("serving is stopped".into());
            }
            let armed =
                solstone_core_backup_runtime::rotation::arm_rotation_parent_sync_fault(root);
            if !armed {
                return Err("rotation publication fault was not armed".into());
            }
            Ok(json!({"armed":true,"checkpoint":"parent_sync"}))
        }
        #[cfg(unix)]
        "serve-rotation-fault-status" => {
            let (armed, fired) =
                solstone_core_backup_runtime::rotation::rotation_parent_sync_fault_observation();
            Ok(json!({"armed":armed,"fired":fired}))
        }
        #[cfg(target_os = "ios")]
        "serve-teardown-interrupt-arm" => {
            if SERVING.lock().map_err(|e| e.to_string())?.is_none() {
                return Err("serving is stopped".into());
            }
            use solstone_core_offload::teardown::{
                OwnedTeardownInterrupt, arm_owned_teardown_interrupt,
                owned_teardown_interrupt_observation,
            };
            let boundary = OwnedTeardownInterrupt::AfterClear;
            arm_owned_teardown_interrupt(root, boundary);
            let observed = owned_teardown_interrupt_observation(root, boundary);
            if !observed.armed() || observed.fired() {
                return Err("teardown interruption was not armed".into());
            }
            Ok(json!({"armed":true,"checkpoint":"after_confirmed_clear"}))
        }
        #[cfg(target_os = "ios")]
        "serve-teardown-interrupt-status" => {
            use solstone_core_offload::teardown::{
                OwnedTeardownInterrupt, owned_teardown_interrupt_observation,
            };
            let observed =
                owned_teardown_interrupt_observation(root, OwnedTeardownInterrupt::AfterClear);
            Ok(json!({"armed":observed.armed(),"fired":observed.fired()}))
        }
        "serve-status" | "serve-owner" => {
            let serving = SERVING.lock().map_err(|e| e.to_string())?;
            let serving = serving.as_ref().ok_or("serving is stopped")?;
            match op {
                "serve-status" => Ok(serving.status()),
                _ => {
                    let payload: Value =
                        serde_json::from_str(v["payload"].as_str().ok_or("missing owner request")?)
                            .map_err(|e| e.to_string())?;
                    let response = serving.serving.request_owner(
                        payload["method"].as_str().ok_or("missing method")?,
                        payload["uri"].as_str().ok_or("missing URI")?,
                        payload["body"].as_str().unwrap_or(""),
                    )?;
                    Ok(json!({"status":response.status,"body":response.body}))
                }
            }
        }
        "serve-custody" => {
            let requested = v["root"].as_str().ok_or("missing custody authority")?;
            if std::fs::canonicalize(requested).map_err(|e| e.to_string())? != p {
                return Err("custody journal does not match admitted root".into());
            }
            Ok(json!({"root_validated":true}))
        }
        "serve-inventory" => {
            if SERVING.lock().map_err(|e| e.to_string())?.is_some() {
                return Err("stop serving before snapshotting files".into());
            }
            let mut files = serde_json::Map::new();
            for name in [
                "facets/work/activities/20261012.jsonl",
                "link/state.json",
                "link/authorized_clients.json",
            ] {
                files.insert(
                    name.into(),
                    json!(std::fs::read_to_string(p.join(name)).ok()),
                );
            }
            fn storage(path: &Path) -> Result<(u64, u64), String> {
                let mut sum = (0, 0);
                for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let metadata = entry.metadata().map_err(|e| e.to_string())?;
                    let count = if metadata.is_dir() {
                        storage(&entry.path())?
                    } else {
                        (1, metadata.len())
                    };
                    sum.0 += count.0;
                    sum.1 += count.1;
                }
                Ok(sum)
            }
            let (count, bytes) = storage(p)?;
            Ok(json!({"files":files,"storage_files":count,"storage_bytes":bytes}))
        }
        "audio-inventory" => {
            if SERVING.lock().map_err(|e| e.to_string())?.is_some() {
                return Err("stop serving before snapshotting audio files".into());
            }
            fn collect(root: &Path, path: &Path, files: &mut Vec<Value>) -> Result<(), String> {
                for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let kind = entry.file_type().map_err(|e| e.to_string())?;
                    if kind.is_symlink() {
                        if entry.path() == root.join("health/callosum.sock") {
                            files.push(json!({"path":"health/callosum.sock","socket_alias":
                                std::fs::read_link(entry.path()).map_err(|e| e.to_string())?}));
                            continue;
                        }
                        return Err("unexpected synthetic journal symlink".into());
                    }
                    if kind.is_dir() {
                        collect(root, &entry.path(), files)?;
                    } else if kind.is_file() {
                        let path = entry.path();
                        let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                        let size = file.metadata().map_err(|e| e.to_string())?.len();
                        let mut hash = Sha256::new();
                        std::io::copy(&mut file, &mut hash).map_err(|e| e.to_string())?;
                        let text = (path.starts_with(root.join("chronicle"))
                            && matches!(
                                path.extension().and_then(|e| e.to_str()),
                                Some("jsonl" | "json")
                            ))
                        .then(|| std::fs::read_to_string(&path))
                        .transpose()
                        .map_err(|e| e.to_string())?;
                        files.push(
                            json!({"path":path.strip_prefix(root).map_err(|e| e.to_string())?,
                            "bytes":size,"sha256":format!("{:x}",hash.finalize()),"text":text}),
                        );
                    }
                }
                Ok(())
            }
            let mut files = Vec::new();
            collect(p, p, &mut files)?;
            Ok(json!({"files":files}))
        }
        "serve-export-admit" => {
            if SERVING.lock().map_err(|e| e.to_string())?.is_some() {
                return Err("stop serving before exporting a snapshot".into());
            }
            Ok(json!({"validated":true}))
        }
        "prepare-export" => {
            if SERVING.lock().map_err(|e| e.to_string())?.is_some() {
                return Err("stop serving before exporting a snapshot".into());
            }
            let requested = v["root"].as_str().ok_or("missing export authority")?;
            if std::fs::canonicalize(requested).map_err(|e| e.to_string())? != p {
                return Err("export journal does not match admitted root".into());
            }
            // Release the actor reader before Swift snapshots the database and WAL.
            // Serving is stopped; the serial work queue excludes concurrent IO.
            QUERY.lock().map_err(|e| e.to_string())?.take();
            Ok(json!({"validated":true}))
        }
        "open" => Ok(json!({"root":p,"identity":root.identity().observation_id()})),
        "seed" => {
            let count = v["count"].as_u64().unwrap_or(128).clamp(1, 4096);
            let dir = p.join("chronicle/20261012/talents");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let bound = JournalRoot::open(&dir).map_err(|e| e.to_string())?;
            for i in 0..count {
                create(
                    &bound,
                    &format!("fixture{i:04}.md"),
                    fixture(i, 0).as_bytes(),
                )?;
            }
            replace(root, "checkpoint.bin", b"prior complete checkpoint")?;
            Ok(json!({"seeded":count}))
        }
        "index" => {
            let r = solstone_core_indexer_store::scan::scan_journal(p, true)
                .map_err(|e| e.to_string())?;
            Ok(
                json!({"indexed":r.indexed,"failed":r.failed,"warnings":r.warnings,"query":query(p)?}),
            )
        }
        "query" => query(p),
        "inventory" => inventory(p),
        "create" => {
            create(root, "exclusive.bin", b"create once")?;
            Ok(json!({"created":true}))
        }
        "crash-write" => {
            let _ = atomic::run_with_bound_publication_barrier(
                BoundPublicationPrimitive::Rename,
                1,
                || std::process::abort(),
                || replace(root, "checkpoint.bin", b"unpublished replacement"),
            );
            Err("write fault did not fire".into())
        }
        "commit-disconnect" => {
            create(root, "committed.bin", b"committed before reply loss")?;
            std::process::abort()
        }
        "recover" => {
            let before = inventory(p)?;
            replace(root, "checkpoint.bin", b"recovered complete checkpoint")?;
            Ok(json!({"before":before,"query":query(p)?}))
        }
        "binding-control" => {
            let moved = p.with_extension("binding-control");
            if moved.exists() {
                return Err("binding control destination exists".into());
            }
            std::fs::rename(p, &moved).map_err(|e| e.to_string())?;
            std::fs::create_dir(p).map_err(|e| e.to_string())?;
            let refusal = replace(root, "must-not-publish", b"no").is_err();
            let absent =
                !p.join("must-not-publish").exists() && !moved.join("must-not-publish").exists();
            std::fs::remove_dir(p).map_err(|e| e.to_string())?;
            std::fs::rename(&moved, p).map_err(|e| e.to_string())?;
            root.revalidate_canonical_binding()
                .map_err(|e| e.to_string())?;
            Ok(json!({"refused":refusal,"both_absent":absent}))
        }
        "active" | "control" => {
            CANCEL.store(false, Ordering::SeqCst);
            ACTIVE.store(true, Ordering::SeqCst);
            struct ActiveGuard;
            impl Drop for ActiveGuard {
                fn drop(&mut self) {
                    ACTIVE.store(false, Ordering::SeqCst);
                }
            }
            let _active = ActiveGuard;
            let seconds = v["seconds"].as_u64().unwrap_or(10).clamp(1, 120);
            let mode = v["mode"].as_str().unwrap_or("reopen");
            // Controls remove the normal actor reader, then vary only reader lifetime.
            QUERY.lock().map_err(|e| e.to_string())?.take();
            // Retain a store reader while still calling the one-shot search API.
            let retained = if mode == "retained" {
                Some(
                    solstone_core_indexer_store::db::open_index_reader(p)
                        .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            if let Some(c) = &retained {
                c.query_row("SELECT count(*) FROM chunks", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| e.to_string())?;
            }
            let start = Instant::now();
            let mut n = 0;
            while start.elapsed() < Duration::from_secs(seconds) && !CANCEL.load(Ordering::SeqCst) {
                query_with(p, 128, mode == "session")?;
                n += 1;
            }
            QUERY.lock().map_err(|e| e.to_string())?.take();
            Ok(
                json!({"queries":n,"seconds":start.elapsed().as_secs_f64(),"cancelled":CANCEL.load(Ordering::SeqCst),"mode":mode}),
            )
        }
        "mixed" => {
            let i = v["count"].as_u64().unwrap_or(0) % 32;
            let revision = v["revision"].as_u64().unwrap_or(1);
            let bound = JournalRoot::open(&p.join("chronicle/20261012/talents"))
                .map_err(|e| e.to_string())?;
            replace(
                &bound,
                &format!("fixture{i:04}.md"),
                fixture(i, revision).as_bytes(),
            )?;
            replace(
                root,
                "checkpoint.bin",
                format!("mixed revision {revision}").as_bytes(),
            )?;
            let r = solstone_core_indexer_store::scan::scan_journal(p, true)
                .map_err(|e| e.to_string())?;
            Ok(
                json!({"revision":revision,"failed":r.failed,"warnings":r.warnings,"query":query_limit(p,8)?}),
            )
        }
        _ => Err(format!("unknown operation {op}")),
    }
}
/// # Safety
/// Input is a live NUL-terminated UTF-8 string; free the returned string once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_runtime(input: *const c_char) -> *mut c_char {
    let start = Instant::now();
    let result = std::panic::catch_unwind(|| {
        install_door_warnings();
        if input.is_null() {
            return Err("null request".into());
        }
        let s = unsafe { CStr::from_ptr(input) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let request = serde_json::from_str(s).map_err(|e| e.to_string())?;
        #[cfg(target_os = "ios")]
        {
            // GCD worker stacks are too small for the debug build's nested
            // router/TLS futures. Keep the caller's serial admission while
            // executing native work on an explicitly bounded Rust stack.
            let worker = std::thread::Builder::new()
                .name("journal-native-call".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || run(request))
                .map_err(|e| e.to_string())?;
            match worker.join() {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        #[cfg(not(target_os = "ios"))]
        run(request)
    });
    let body = match result {
        Ok(Ok(v)) => json!({"ok":true,"value":v}),
        Ok(Err(e)) => json!({"ok":false,"error":e}),
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            json!({"ok":false,"error":format!("native panic; reopen helper: {detail}")})
        }
    };
    CString::new(json!({"pid":std::process::id(),"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"result":body}).to_string()).unwrap().into_raw()
}
/// # Safety
/// Pointer was returned by journal_runtime and has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn journal_runtime_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}

static CANCEL_GEN: AtomicU64 = AtomicU64::new(0);
static NEXT_APPRAISAL_ATTEMPT: AtomicU64 = AtomicU64::new(1);
static VERIFIER_TRANSPORT: OnceLock<extern "C" fn(*const c_char) -> *mut c_char> = OnceLock::new();

const MAX_APPRAISAL_REQUEST_BYTES: usize = 1_114_112;
const MAX_APPRAISAL_RESPONSE_BYTES: usize = 4_194_304;
const MAX_APPRAISAL_VALUE_BYTES: usize = 2_097_152;
const MAX_APPRAISAL_DIAGNOSTIC_BYTES: usize = 4_096;

struct ActiveAppraisal {
    id: u64,
    generation: u64,
    state: solstone_core_spp_attest::ios_appraisal::AppraisalStateMachine,
}

static ACTIVE_APPRAISAL: Mutex<Option<ActiveAppraisal>> = Mutex::new(None);
static IOS_VERIFIER_ADMISSION_DIAGNOSTIC: Mutex<Option<String>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_verifier_admit() -> bool {
    match solstone_core_spp_attest::ios_service_tools::admit_ios_verifier() {
        Ok(_) => {
            if let Ok(mut diagnostic) = IOS_VERIFIER_ADMISSION_DIAGNOSTIC.lock() {
                *diagnostic = Some("admitted".into());
            }
            true
        }
        Err(error) => {
            if let Ok(mut diagnostic) = IOS_VERIFIER_ADMISSION_DIAGNOSTIC.lock() {
                let mut message = error.to_string();
                let mut end = message.len().min(MAX_APPRAISAL_DIAGNOSTIC_BYTES);
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                message.truncate(end);
                *diagnostic = Some(message);
            }
            false
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_verifier_admission_diagnostic() -> *mut c_char {
    let diagnostic = IOS_VERIFIER_ADMISSION_DIAGNOSTIC
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .unwrap_or_else(|| "admission not attempted".into());
    CString::new(diagnostic)
        .unwrap_or_else(|_| CString::new("invalid admission diagnostic").unwrap())
        .into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_verifier_register_callback(
    transport: Option<extern "C" fn(*const c_char) -> *mut c_char>,
) -> bool {
    let Some(transport_fn) = transport else {
        return false;
    };
    if solstone_core_spp_attest::ios_appraisal::register_ios_gpu_appraisal_callback(
        rust_appraisal_bridge,
    )
    .is_err()
    {
        return false;
    }
    match VERIFIER_TRANSPORT.get() {
        Some(existing) => std::ptr::fn_addr_eq(*existing, transport_fn),
        None => {
            VERIFIER_TRANSPORT.set(transport_fn).is_ok()
                || VERIFIER_TRANSPORT
                    .get()
                    .is_some_and(|existing| std::ptr::fn_addr_eq(*existing, transport_fn))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_verifier_cancel() {
    cancel_appraisal(&ACTIVE_APPRAISAL, &CANCEL_GEN);
}

fn cancel_appraisal(active_appraisal: &Mutex<Option<ActiveAppraisal>>, generation: &AtomicU64) {
    cancel_appraisal_after(active_appraisal, generation, || {});
}

fn cancel_appraisal_after(
    active_appraisal: &Mutex<Option<ActiveAppraisal>>,
    generation: &AtomicU64,
    before_lock: impl FnOnce(),
) {
    before_lock();
    match active_appraisal.lock() {
        Ok(mut guard) => {
            // Linearize cancellation with the final publication check. A
            // publisher holding this lock completes first; otherwise this
            // generation change makes its accepted reply unpublishable.
            generation.fetch_add(1, Ordering::SeqCst);
            if let Some(ref mut active) = *guard {
                let _ = active
                    .state
                    .handle_event(solstone_core_spp_attest::ios_appraisal::AppraisalEvent::Cancel);
            }
        }
        Err(_) => {
            generation.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn journal_ios_verifier_drain_ready() -> bool {
    if let Ok(guard) = ACTIVE_APPRAISAL.lock() {
        if let Some(ref active) = *guard {
            active.state.drain_ready()
        } else {
            true
        }
    } else {
        false
    }
}

fn rust_appraisal_bridge(
    req: &solstone_core_spp_attest::ios_appraisal::IosGpuAppraisalRequest<'_>,
) -> Result<
    solstone_core_spp_attest::ios_appraisal::IosAppraisalReply,
    solstone_core_spp_attest::error::GpuAppraisalReason,
> {
    use solstone_core_spp_attest::error::GpuAppraisalReason;
    use solstone_core_spp_attest::ios_appraisal::{
        AppraisalEvent, AppraisalStateMachine, IosAppraisalReply,
    };

    let started = Instant::now();
    let generation = CANCEL_GEN.load(Ordering::SeqCst);
    let transport_fn = *VERIFIER_TRANSPORT
        .get()
        .ok_or(GpuAppraisalReason::NvattestUnavailable)?;
    let attempt = NEXT_APPRAISAL_ATTEMPT.fetch_add(1, Ordering::SeqCst);
    if attempt == 0 {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }

    {
        let mut guard = ACTIVE_APPRAISAL
            .lock()
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        if guard
            .as_ref()
            .is_some_and(|active| !active.state.drain_ready())
        {
            return Err(GpuAppraisalReason::GpuAppraisalFailed);
        }
        if CANCEL_GEN.load(Ordering::SeqCst) != generation {
            return Err(GpuAppraisalReason::GpuAppraisalFailed);
        }
        let mut state = AppraisalStateMachine::new();
        state
            .handle_event(AppraisalEvent::Start)
            .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
        *guard = Some(ActiveAppraisal {
            id: attempt,
            generation,
            state,
        });
    }

    let hex_nonce = req
        .nonce
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let hex_proofs = req
        .proofs
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let evidence_json_val: Value = match serde_json::from_slice(req.evidence_json) {
        Ok(value) => value,
        Err(_) => {
            retire_active_before_transport(attempt, "evidence JSON was invalid")?;
            return Err(GpuAppraisalReason::GpuAppraisalFailed);
        }
    };

    let request_val = json!({
        "profile_fingerprint": req.profile_fingerprint,
        "nonce_hex": hex_nonce,
        "evidence_json": evidence_json_val,
        "proofs_hex": hex_proofs,
        "verification_time_unix": req.verification_time_unix,
        "budget_ms": req.budget_ms,
        "bridge_attempt": attempt,
    });
    let request_str = request_val.to_string();
    if request_str.len() > MAX_APPRAISAL_REQUEST_BYTES {
        retire_active_before_transport(attempt, "appraisal request exceeded byte limit")?;
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    let request_cstr = match CString::new(request_str) {
        Ok(value) => value,
        Err(_) => {
            retire_active_before_transport(attempt, "appraisal request contained NUL")?;
            return Err(GpuAppraisalReason::GpuAppraisalFailed);
        }
    };

    // Call Swift transport
    let response_ptr = transport_fn(request_cstr.as_ptr());
    if response_ptr.is_null() {
        update_active_attempt(attempt, |state| {
            let event = if CANCEL_GEN.load(Ordering::SeqCst) != generation {
                AppraisalEvent::Cancel
            } else {
                AppraisalEvent::Malformed("null response from transport".into())
            };
            let _ = state.handle_event(event);
        })?;
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }

    let response_len = unsafe { libc::strnlen(response_ptr, MAX_APPRAISAL_RESPONSE_BYTES + 1) };
    if response_len > MAX_APPRAISAL_RESPONSE_BYTES {
        unsafe { libc::free(response_ptr.cast::<libc::c_void>()) };
        update_active_attempt(attempt, |state| {
            let _ = state.handle_event(AppraisalEvent::Malformed(
                "transport response exceeded byte limit".into(),
            ));
        })?;
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    let response_bytes = unsafe {
        let bytes = std::slice::from_raw_parts(response_ptr.cast::<u8>(), response_len).to_vec();
        libc::free(response_ptr as *mut libc::c_void);
        bytes
    };

    let transport_rep: Value = match serde_json::from_slice(&response_bytes) {
        Ok(v) => v,
        Err(e) => {
            update_active_attempt(attempt, |state| {
                let event = if CANCEL_GEN.load(Ordering::SeqCst) != generation {
                    AppraisalEvent::Cancel
                } else {
                    AppraisalEvent::Malformed(e.to_string())
                };
                let _ = state.handle_event(event);
            })?;
            return Err(GpuAppraisalReason::GpuAppraisalFailed);
        }
    };

    if transport_rep.get("bridge_attempt").and_then(Value::as_u64) != Some(attempt) {
        update_active_attempt(attempt, |state| {
            let _ = state.handle_event(AppraisalEvent::Malformed(
                "transport attempt identity mismatch".into(),
            ));
        })?;
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }

    let cancelled = CANCEL_GEN.load(Ordering::SeqCst) != generation
        || transport_rep
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let deadline_expired = started.elapsed() >= Duration::from_millis(req.budget_ms)
        || transport_rep
            .get("deadline")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let mut guard = ACTIVE_APPRAISAL
        .lock()
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
    let active = guard
        .as_mut()
        .filter(|active| active.id == attempt && active.generation == generation)
        .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?;
    let sm = &mut active.state;

    // Apply events according to transport report:
    // Launched, then cancel/malformed/disconnect/reply, then Exited
    let pid = transport_rep
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
        .filter(|raw| *raw > 0);
    if let Some(pid) = pid {
        let _ = sm.handle_event(AppraisalEvent::Launched { pid });
    }

    if cancelled {
        let _ = sm.handle_event(AppraisalEvent::Cancel);
    } else if deadline_expired {
        let _ = sm.handle_event(AppraisalEvent::Deadline);
    } else if let Some(err_msg) = transport_rep.get("malformed").and_then(Value::as_str) {
        let diagnostic = if err_msg.len() <= MAX_APPRAISAL_DIAGNOSTIC_BYTES {
            err_msg.to_string()
        } else {
            "transport diagnostic exceeded byte limit".into()
        };
        let _ = sm.handle_event(AppraisalEvent::Malformed(diagnostic));
    } else if transport_rep
        .get("disconnect")
        .or_else(|| transport_rep.get("disconnected"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let _ = sm.handle_event(AppraisalEvent::Disconnect);
    } else if let Some(reply_obj) = transport_rep.get("reply") {
        let returncode = reply_obj
            .get("returncode")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok());
        let result_code = reply_obj
            .get("result_code")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok());
        let result_message = reply_obj
            .get("result_message")
            .and_then(Value::as_str)
            .filter(|message| message.len() <= MAX_APPRAISAL_DIAGNOSTIC_BYTES);
        let claims = reply_obj.get("claims").cloned();
        let detached_eat = reply_obj.get("detached_eat").cloned();

        match (
            returncode,
            result_code,
            result_message,
            claims,
            detached_eat,
        ) {
            (
                Some(returncode),
                Some(result_code),
                Some(result_message),
                Some(claims),
                Some(detached_eat),
            ) if serde_json::to_vec(&claims)
                .is_ok_and(|bytes| bytes.len() <= MAX_APPRAISAL_VALUE_BYTES)
                && serde_json::to_vec(&detached_eat)
                    .is_ok_and(|bytes| bytes.len() <= MAX_APPRAISAL_VALUE_BYTES) =>
            {
                let reply = IosAppraisalReply {
                    returncode,
                    result_code,
                    result_message: result_message.to_string(),
                    claims,
                    detached_eat,
                };
                let _ = sm.handle_event(AppraisalEvent::Reply(reply));
            }
            _ => {
                let _ = sm.handle_event(AppraisalEvent::Malformed(
                    "helper reply schema or byte limit invalid".into(),
                ));
            }
        }
    } else {
        let _ = sm.handle_event(AppraisalEvent::Malformed(
            "transport report missing terminal outcome".into(),
        ));
    }

    if let Some(pid) = pid
        && transport_rep
            .get("exited")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        let _ = sm.handle_event(AppraisalEvent::Exited { pid });
    } else if pid.is_none()
        && transport_rep
            .get("retired_without_launch")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        let _ = sm.handle_event(AppraisalEvent::RetiredWithoutLaunch);
    }

    let cancelled_at_publication = CANCEL_GEN.load(Ordering::SeqCst) != generation;
    if cancelled_at_publication {
        let _ = sm.handle_event(AppraisalEvent::Cancel);
    }

    let term = sm.terminal_result().cloned();
    appraisal_publication_result(
        term,
        cancelled_at_publication,
        started.elapsed() >= Duration::from_millis(req.budget_ms),
    )
}

fn appraisal_publication_result(
    term: Option<solstone_core_spp_attest::ios_appraisal::AppraisalTerminalResult>,
    cancelled: bool,
    deadline_expired: bool,
) -> Result<
    solstone_core_spp_attest::ios_appraisal::IosAppraisalReply,
    solstone_core_spp_attest::error::GpuAppraisalReason,
> {
    use solstone_core_spp_attest::error::GpuAppraisalReason;
    use solstone_core_spp_attest::ios_appraisal::AppraisalTerminalResult;

    if cancelled || deadline_expired {
        return Err(GpuAppraisalReason::GpuAppraisalFailed);
    }
    match term {
        Some(AppraisalTerminalResult::Accepted(reply)) => Ok(reply),
        _ => Err(GpuAppraisalReason::GpuAppraisalFailed),
    }
}

fn update_active_attempt(
    attempt: u64,
    update: impl FnOnce(&mut solstone_core_spp_attest::ios_appraisal::AppraisalStateMachine),
) -> Result<(), solstone_core_spp_attest::error::GpuAppraisalReason> {
    use solstone_core_spp_attest::error::GpuAppraisalReason;
    let mut guard = ACTIVE_APPRAISAL
        .lock()
        .map_err(|_| GpuAppraisalReason::GpuAppraisalFailed)?;
    let active = guard
        .as_mut()
        .filter(|active| active.id == attempt)
        .ok_or(GpuAppraisalReason::GpuAppraisalFailed)?;
    update(&mut active.state);
    Ok(())
}

fn retire_active_before_transport(
    attempt: u64,
    diagnostic: &str,
) -> Result<(), solstone_core_spp_attest::error::GpuAppraisalReason> {
    use solstone_core_spp_attest::ios_appraisal::AppraisalEvent;
    update_active_attempt(attempt, |state| {
        let _ = state.handle_event(AppraisalEvent::Malformed(diagnostic.into()));
        let _ = state.handle_event(AppraisalEvent::RetiredWithoutLaunch);
    })
}

#[cfg(test)]
mod processing_admission_tests {
    use super::*;

    #[test]
    fn stop_during_startup_cannot_reopen_the_previous_lifetime() {
        let state = AtomicU64::new(1);
        let queued_start = json!({"admissionGeneration":1});
        let generation = admit_processing(&queued_start, &state).unwrap();
        close_processing_admission(&state);
        assert!(!open_processing_admission(&state, generation));
        assert!(admit_processing(&queued_start, &state).is_err());
        let current = state.load(Ordering::SeqCst);
        assert!(open_processing_admission(&state, current));
        assert_eq!(state.load(Ordering::SeqCst) & 1, 0);
        assert!(admit_processing(&queued_start, &state).is_err());
    }

    #[test]
    fn queued_work_does_not_adopt_a_restart() {
        let state = AtomicU64::new(1);
        assert!(open_processing_admission(&state, 1));
        let queued_work = json!({"admissionGeneration":0});
        assert_eq!(admit_processing(&queued_work, &state).unwrap(), 0);
        close_processing_admission(&state);
        assert!(admit_processing(&queued_work, &state).is_err());
        let restart = state.load(Ordering::SeqCst);
        assert!(open_processing_admission(&state, restart));
        assert!(admit_processing(&queued_work, &state).is_err());
        assert!(admit_processing(&json!({}), &state).is_err());
    }
}

#[cfg(test)]
mod appraisal_publication_tests {
    use super::*;
    use solstone_core_spp_attest::ios_appraisal::{AppraisalTerminalResult, IosAppraisalReply};
    use std::sync::{Arc, mpsc};

    fn accepted() -> Option<AppraisalTerminalResult> {
        Some(AppraisalTerminalResult::Accepted(IosAppraisalReply {
            returncode: 0,
            result_code: 0,
            result_message: "Ok".into(),
            claims: json!([]),
            detached_eat: json!([]),
        }))
    }

    #[test]
    fn cancellation_after_terminal_acceptance_still_refuses_publication() {
        assert!(appraisal_publication_result(accepted(), true, false).is_err());
    }

    #[test]
    fn deadline_after_terminal_acceptance_still_refuses_publication() {
        assert!(appraisal_publication_result(accepted(), false, true).is_err());
    }

    #[test]
    fn accepted_result_publishes_only_inside_both_final_fences() {
        assert!(appraisal_publication_result(accepted(), false, false).is_ok());
    }

    #[test]
    fn cancellation_generation_is_serialized_with_publication_lock() {
        let active = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(41));
        let publication = active.lock().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let active_for_cancel = Arc::clone(&active);
        let generation_for_cancel = Arc::clone(&generation);
        let cancel = std::thread::spawn(move || {
            cancel_appraisal_after(&active_for_cancel, &generation_for_cancel, || {
                entered_tx.send(()).unwrap();
            });
        });
        entered_rx.recv().unwrap();
        // The hook is the exact lock-attempt boundary. If the generation
        // increment moves before the publication lock, this assertion fails
        // without scheduler timing or a wall-clock wait.
        assert_eq!(generation.load(Ordering::SeqCst), 41);
        drop(publication);
        cancel.join().unwrap();
        assert_eq!(generation.load(Ordering::SeqCst), 42);
    }
}
