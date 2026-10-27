// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use journal_mobile_serving::foreground::ForegroundProcessingHost;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Weak, mpsc};
use std::time::Duration;

type RegistrationHook = Box<dyn FnOnce(&jio::JournalRoot, &solstone_core_import::AdmittedFolder)>;
thread_local! {
    static BEFORE_REGISTRATION: std::cell::RefCell<Option<RegistrationHook>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn before_registration(
    root: &jio::JournalRoot,
    folder: &solstone_core_import::AdmittedFolder,
) {
    let callback = BEFORE_REGISTRATION.with(|hook| hook.borrow_mut().take());
    if let Some(callback) = callback {
        callback(root, folder);
    }
}

fn isolated(name: &str) -> bool {
    isolated_full(&format!("owner_folder::tests::{name}"))
}

pub(crate) fn isolated_full(full: &str) -> bool {
    const CHILD: &str = "FC2_NATIVE_FOLDER_CHILD";
    if std::env::var(CHILD).ok().as_deref() == Some(full) {
        return false;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", full, "--nocapture"])
        .env(CHILD, full)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "native folder control timed out: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(full) && stdout.contains("1 passed; 0 failed"),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

pub(crate) struct Fixture {
    directory: PathBuf,
    pub(crate) root: PathBuf,
    cache: PathBuf,
    page: u64,
    generation: u64,
    host: Weak<ForegroundProcessingHost>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        Self::with_generation(2)
    }

    fn with_generation(generation: u64) -> Self {
        let directory =
            std::env::temp_dir().join(format!("fc2-native-folder-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let root = directory.join("journal");
        let cache = directory.join("cache");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::write(root.join("config/journal.json"),
            r#"{"setup":{"completed_at":1},"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#).unwrap();
        let payload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../core/payload");
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
            unreachable!()
        };
        drop(probe);
        THINKING_GENERATION.store(generation, Ordering::SeqCst);
        let server = journal_mobile_serving::MobileServing::start_with_payload(
            root.clone(),
            address,
            payload.clone(),
        )
        .unwrap();
        let binding = Arc::new(jio::JournalRoot::open(&root).unwrap());
        let processing = Arc::new(AtomicBool::new(false));
        let (factory, sources) = crate::saved_sources::factory(PathBuf::from(
            std::env::var("SOLSTONE_NATIVE_FOLDER_TEST_PDFIUM")
                .expect("maintained host PDFium input required"),
        ));
        let host = Arc::new(
            ForegroundProcessingHost::open_with_imports(
                Arc::clone(&binding),
                &payload,
                Arc::new(move || THINKING_GENERATION.load(Ordering::SeqCst) != generation),
                Arc::clone(&processing),
                server.backup_dependencies(),
                factory,
                false,
                false,
            )
            .unwrap(),
        );
        server
            .import_route()
            .install(Arc::new(
                journal_mobile_serving::import_route::ForegroundImportRoute::new(Arc::clone(&host)),
            ))
            .unwrap();
        *SERVING_STOP.lock().unwrap() = Some(server.stop_handle());
        *PROCESSING_STOP.lock().unwrap() = Some(processing);
        let weak = Arc::downgrade(&host);
        *SERVING.lock().unwrap() = Some(ServingInstance {
            handle: Some(server),
            retirement: None,
            root: binding,
            host: Some(host),
            sources,
        });
        let root_c = CString::new(root.to_str().unwrap()).unwrap();
        let page = unsafe { journal_owner_folder_page_issue(root_c.as_ptr(), generation) };
        assert_ne!(page, 0);
        assert!(!journal_owner_folder_copy_continues());
        Self {
            directory,
            root,
            cache,
            page,
            generation,
            host: weak,
        }
    }

    fn request(&self) -> Value {
        json!({"source":"obsidian","client_item_id":"native-folder-owner",
            "timestamp":"20261014_090000","folder_name":"picked notes",
            "page_authority":self.page,"admission_generation":self.generation})
    }

    pub(crate) fn stop(&self) {
        request_serving_stop();
        SERVING
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .begin_retirement()
            .unwrap();
    }

    pub(crate) fn stopped(&self) {
        assert_eq!(
            retirement::wait_for_process(Duration::from_secs(10)),
            retirement::Outcome::Stopped
        );
        *SERVING.lock().unwrap() = None;
        assert!(
            self.host.upgrade().is_none(),
            "retired native owner still retains host"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

struct Provider {
    worker: std::thread::ThreadId,
    page: u64,
    resource: Rc<()>,
    entered: Option<mpsc::Sender<()>>,
    release: Option<mpsc::Receiver<()>>,
}

unsafe extern "C" fn provider(context: *mut c_void, destination: *const c_char) -> bool {
    let state = unsafe { &mut *context.cast::<Provider>() };
    assert_eq!(std::thread::current().id(), state.worker);
    assert_eq!(Rc::strong_count(&state.resource), 1);
    assert!(journal_owner_folder_copy_continues());
    assert_eq!(journal_owner_folder_page_current(), state.page);
    let destination = Path::new(unsafe { CStr::from_ptr(destination) }.to_str().unwrap());
    fs::create_dir(destination.join("empty")).unwrap();
    fs::write(
        destination.join("note.md"),
        b"# Picked notes\n\ncobalt retained provider note\n",
    )
    .unwrap();
    let modified: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse().unwrap();
    fs::File::open(destination.join("note.md"))
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(
            std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(modified.timestamp().try_into().unwrap()),
        ))
        .unwrap();
    if let Some(entered) = state.entered.take() {
        entered.send(()).unwrap();
        state
            .release
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(
            !journal_owner_folder_copy_continues(),
            "native Stop did not reach provider"
        );
        return false;
    }
    true
}

fn copy(
    cache: &Path,
    request: &Value,
    blocked: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
) -> Value {
    let (entered, release) = blocked.map_or((None, None), |(a, b)| (Some(a), Some(b)));
    let mut state = Provider {
        worker: std::thread::current().id(),
        page: request["page_authority"].as_u64().unwrap(),
        resource: Rc::new(()),
        entered,
        release,
    };
    let cache = CString::new(cache.to_str().unwrap()).unwrap();
    let request = request.to_string();
    let result = unsafe {
        journal_owner_folder_copy(
            cache.as_ptr(),
            request.as_ptr(),
            request.len(),
            (&mut state as *mut Provider).cast(),
            Some(provider),
        )
    };
    let result = unsafe { CString::from_raw(result) };
    assert!(
        !journal_owner_folder_copy_continues(),
        "native worker kept its installed Stop after return"
    );
    serde_json::from_slice(result.as_bytes()).unwrap()
}

fn no_temporary_copy(cache: &Path) {
    assert!(
        fs::read_dir(cache.join("owner-bridge"))
            .unwrap()
            .next()
            .is_none()
    );
}

fn owner_json(method: &str, path: &str, body: Value) -> Value {
    let owner = SERVING.lock().unwrap();
    let response = owner
        .as_ref()
        .unwrap()
        .handle()
        .unwrap()
        .request_owner(method, path, &body.to_string())
        .unwrap();
    assert_eq!(response.status, 200, "{path}: {}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

#[test]
fn unbound_page_at_active_generation_zero_refuses_before_provider_admission() {
    if isolated("unbound_page_at_active_generation_zero_refuses_before_provider_admission") {
        return;
    }
    let fixture = Fixture::with_generation(0);
    let page = journal_owner_folder_page_allocate();
    let mut request = fixture.request();
    request["page_authority"] = json!(page);
    assert_eq!(copy(&fixture.cache, &request, None)["state"], "refused");
    let wrong_root = CString::new(fixture.directory.join("missing").to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe { journal_owner_folder_page_bind(wrong_root.as_ptr(), 0, page) },
        0
    );
    assert_eq!(copy(&fixture.cache, &request, None)["state"], "refused");
    let root = CString::new(fixture.root.to_str().unwrap()).unwrap();
    assert_eq!(
        unsafe { journal_owner_folder_page_bind(root.as_ptr(), 0, page) },
        page
    );
    assert_eq!(copy(&fixture.cache, &request, None)["state"], "saved");
    no_temporary_copy(&fixture.cache);
    fixture.stop();
    fixture.stopped();
}

#[test]
fn delayed_page_binding_cannot_replace_the_newer_owner_document() {
    if isolated("delayed_page_binding_cannot_replace_the_newer_owner_document") {
        return;
    }
    let fixture = Fixture::new();
    let root = CString::new(fixture.root.to_str().unwrap()).unwrap();
    let old = journal_owner_folder_page_allocate();
    let current = journal_owner_folder_page_allocate();
    assert_ne!(old, current);
    assert_eq!(
        unsafe { journal_owner_folder_page_bind(root.as_ptr(), 2, current) },
        current
    );
    assert_eq!(
        unsafe { journal_owner_folder_page_bind(root.as_ptr(), 2, old) },
        0
    );
    journal_owner_folder_page_retire(old);
    let mut request = fixture.request();
    request["page_authority"] = json!(old);
    assert_eq!(copy(&fixture.cache, &request, None)["state"], "refused");
    request["page_authority"] = json!(current);
    request.as_object_mut().unwrap().remove("timestamp");
    let result = copy(&fixture.cache, &request, None);
    assert_eq!(result["state"], "saved", "{result}");
    validate_timestamp(result["import_id"].as_str().unwrap()).unwrap();
    no_temporary_copy(&fixture.cache);
    fixture.stop();
    fixture.stopped();
}

#[test]
fn native_folder_save_preview_start_and_read_use_the_original_service_owner() {
    if isolated("native_folder_save_preview_start_and_read_use_the_original_service_owner") {
        return;
    }
    let fixture = Fixture::new();
    let request = fixture.request();
    let result = copy(&fixture.cache, &request, None);
    assert_eq!(result["state"], "saved", "{result}");
    let id = result["import_id"].as_str().unwrap();
    let directory = fixture.root.join("imports").join(id);
    assert_eq!(
        fs::read(directory.join("tree/note.md")).unwrap(),
        b"# Picked notes\n\ncobalt retained provider note\n"
    );
    assert!(directory.join("tree/empty").is_dir());
    let metadata = solstone_core_import::read_import_metadata(&fixture.root, id).unwrap();
    assert_eq!(metadata["source_hint"], "obsidian");
    assert_eq!(metadata["timestamp_detection_method"], "path_fallback");
    assert_eq!(metadata["client_item_id"], "native-folder-owner");
    assert!(solstone_core_import::get_attempt_facts(&metadata).is_none());
    no_temporary_copy(&fixture.cache);
    let mut start = json!({"path": result["value"]["path"],
        "timestamp": result["value"]["source_timestamp"], "import_id": id});
    let preview = owner_json("POST", "/app/import/api/preview", start.clone());
    assert!(preview["preview_token"].is_string(), "{preview}");
    assert!(
        solstone_core_import::get_attempt_facts(
            &solstone_core_import::read_import_metadata(&fixture.root, id).unwrap()
        )
        .is_none()
    );
    start["preview_token"] = preview["preview_token"].clone();
    owner_json("POST", "/app/import/api/start", start.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let detail = owner_json("GET", &format!("/app/import/api/{id}"), Value::Null);
        if detail["status"] == "success" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "folder import did not complete: {detail}"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
    let facts = solstone_core_import::get_attempt_facts(
        &solstone_core_import::read_import_metadata(&fixture.root, id).unwrap(),
    )
    .unwrap();
    assert_eq!(facts.state, solstone_core_import::AttemptState::Completed);
    assert_eq!(facts.generation, 1);
    let replay = owner_json("POST", "/app/import/api/start", start);
    assert_eq!(replay["replay"], true, "{replay}");
    let read = owner_json(
        "GET",
        "/app/transcripts/api/read/20261014?transcripts=1",
        Value::Null,
    );
    assert!(
        read["markdown"]
            .as_str()
            .is_some_and(|text| text.contains("cobalt retained provider note")),
        "{read}"
    );
    SERVING
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .handle()
        .unwrap()
        .index()
        .unwrap();
    let search = owner_json("GET", "/app/search/api/search?q=cobalt", Value::Null);
    assert!(
        search["days"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|day| day["results"].as_array().unwrap())
            .any(|hit| hit["day"] == "20261014"
                && hit["path"]
                    .as_str()
                    .is_some_and(|path| path.starts_with("20261014/import.obsidian/"))
                && hit["text"].as_str().is_some_and(
                    |text| text.contains("cobalt") && text.contains("retained provider note")
                )),
        "{search}"
    );
    assert_eq!(
        fs::read(directory.join("tree/note.md")).unwrap(),
        b"# Picked notes\n\ncobalt retained provider note\n"
    );
    fixture.stop();
    fixture.stopped();
}

#[test]
fn failed_registration_reports_actual_saved_custody_without_undoing_originals() {
    if isolated("failed_registration_reports_actual_saved_custody_without_undoing_originals") {
        return;
    }
    let fixture = Fixture::new();
    BEFORE_REGISTRATION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|root, folder| {
            let directory = root
                .canonical_path()
                .join("imports")
                .join(folder.import_id.as_str());
            assert!(directory.join("folder-custody.json").is_file());
            fs::write(directory.join("import.json"), b"[]").unwrap();
        }))
    });
    let result = copy(&fixture.cache, &fixture.request(), None);
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["state"], "registration_failed", "{result}");
    assert_eq!(result["custody"], "saved", "{result}");
    let id = result["import_id"].as_str().unwrap();
    assert!(validate_timestamp(id).is_ok());
    let directory = fixture.root.join("imports").join(id);
    assert_eq!(
        fs::read(directory.join("tree/note.md")).unwrap(),
        b"# Picked notes\n\ncobalt retained provider note\n"
    );
    assert!(directory.join("tree/empty").is_dir());
    assert!(directory.join("folder-custody.json").is_file());
    assert_eq!(fs::read(directory.join("import.json")).unwrap(), b"[]");
    no_temporary_copy(&fixture.cache);
    fixture.stop();
    fixture.stopped();
}

#[test]
fn retired_page_refuses_before_provider_or_native_admission() {
    if isolated("retired_page_refuses_before_provider_or_native_admission") {
        return;
    }
    let fixture = Fixture::new();
    journal_owner_folder_page_retire(fixture.page);
    let result = copy(&fixture.cache, &fixture.request(), None);
    assert_eq!(result["state"], "refused", "{result}");
    assert!(!fixture.root.join("imports").exists());
    assert!(!fixture.cache.join("owner-bridge").exists());
    fixture.stop();
    fixture.stopped();
}

#[test]
fn native_stop_and_lost_delivery_keep_provider_cleanup_before_owner_retirement() {
    if isolated("native_stop_and_lost_delivery_keep_provider_cleanup_before_owner_retirement") {
        return;
    }
    let fixture = Fixture::new();
    let cache = fixture.cache.clone();
    let request = fixture.request();
    let (entered, observed) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let (send, abandoned) = mpsc::channel();
    drop(abandoned);
    let worker = std::thread::spawn(move || {
        let result = copy(&cache, &request, Some((entered, held)));
        no_temporary_copy(&cache);
        assert_eq!(result["state"], "refused", "{result}");
        let _ = send.send(result);
    });
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    fixture.stop();
    assert_eq!(
        retirement::wait_for_process(Duration::from_millis(100)),
        retirement::Outcome::Pending
    );
    assert!(fixture.host.upgrade().is_some());
    release.send(()).unwrap();
    worker.join().unwrap();
    fixture.stopped();
    assert!(!fixture.root.join("imports").exists());
    no_temporary_copy(&fixture.cache);
}
