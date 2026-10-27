// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use journal_mobile_serving::foreground::ForegroundProcessingHost;
use std::fs;
use std::rc::Rc;
use std::sync::mpsc;

fn isolated(name: &str) -> bool {
    let full = format!("owner_folder::tests::{name}");
    isolated_full(&full)
}

pub(crate) fn isolated_full(full: &str) -> bool {
    const CHILD: &str = "FC2_IOS_FOLDER_CHILD";
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
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "iOS native folder control timed out: {}{}",
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
    pub(crate) authority: u64,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("fc2-ios-folder-{}", std::process::id()));
        let root = directory.join("journal");
        let cache = directory.join("cache");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::write(root.join("config/journal.json"), r#"{"setup":{"completed_at":1},"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#).unwrap();
        let payload = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../core/payload");
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let std::net::SocketAddr::V4(address) = probe.local_addr().unwrap() else {
            unreachable!()
        };
        drop(probe);
        PROCESSING_GENERATION.store(0, Ordering::SeqCst);
        let server = journal_mobile_serving::MobileServing::start_with_payload(
            root.clone(),
            address,
            payload.clone(),
        )
        .unwrap();
        let binding = server.backup_dependencies().retained_root().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (factory, sources) = crate::saved_sources::factory(PathBuf::from(
            std::env::var("SOLSTONE_NATIVE_FOLDER_TEST_PDFIUM")
                .expect("maintained host PDFium input required"),
        ));
        let host = Arc::new(
            ForegroundProcessingHost::open_with_imports(
                binding,
                &payload,
                Arc::new(|| PROCESSING_GENERATION.load(Ordering::SeqCst) != 0),
                Arc::clone(&stop),
                server.backup_dependencies(),
                factory,
                true,
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
        *PROCESSING_STOP.lock().unwrap() = Some(stop);
        *SERVING.lock().unwrap() = Some(ServingInstance {
            serving: server,
            processing: Some(host),
            sources,
        });
        Self {
            directory,
            root,
            cache,
            authority: owner_response::journal_ios_owner_authority_issue(),
        }
    }
    pub(crate) fn request(&self) -> Value {
        json!({"source":"obsidian","folder_name":"Picked notes","client_item_id":"ios-native-folder", "authority":self.authority,"admissionGeneration":0})
    }
    pub(crate) fn finish(&self) {
        journal_ios_processing_request_stop();
        let mut serving = SERVING.lock().unwrap();
        serving.as_ref().unwrap().drain().unwrap();
        serving.take();
        owner_response::journal_ios_owner_authority_retire(self.authority);
    }
    fn empty_cache(&self) {
        assert!(
            fs::read_dir(self.cache.join("owner-bridge"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct Provider {
    worker: std::thread::ThreadId,
    resource: Rc<()>,
    blocked: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    corrupt_config: Option<PathBuf>,
}
unsafe extern "C" fn provider(context: *mut c_void, destination: *const c_char) -> bool {
    let context = unsafe { &mut *context.cast::<Provider>() };
    assert_eq!(std::thread::current().id(), context.worker);
    assert_eq!(Rc::strong_count(&context.resource), 1);
    assert!(journal_ios_owner_folder_copy_continues());
    let destination = Path::new(unsafe { CStr::from_ptr(destination) }.to_str().unwrap());
    fs::create_dir(destination.join("empty")).unwrap();
    let note = destination.join("note.md");
    fs::write(&note, b"# Picked notes\n\ncobalt iOS retained folder\n").unwrap();
    let instant: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse().unwrap();
    fs::File::open(note)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(
            std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(instant.timestamp().try_into().unwrap()),
        ))
        .unwrap();
    if let Some(path) = &context.corrupt_config {
        fs::write(path, b"{registration-config-control").unwrap();
    }
    if let Some((entered, release)) = context.blocked.take() {
        entered.send(()).unwrap();
        release.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!journal_ios_owner_folder_copy_continues());
    }
    true // A late success must not bypass original Stop after provider return.
}
fn copy(
    cache: &Path,
    request: &Value,
    blocked: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    corrupt_config: Option<PathBuf>,
) -> Value {
    let cache = CString::new(cache.to_str().unwrap()).unwrap();
    let request = request.to_string();
    let mut context = Provider {
        worker: std::thread::current().id(),
        resource: Rc::new(()),
        blocked,
        corrupt_config,
    };
    let raw = unsafe {
        journal_ios_owner_folder_copy(
            cache.as_ptr(),
            request.as_ptr(),
            request.len(),
            (&mut context as *mut Provider).cast(),
            Some(provider),
        )
    };
    let raw = unsafe { CString::from_raw(raw) };
    assert!(!journal_ios_owner_folder_copy_continues());
    serde_json::from_slice(raw.as_bytes()).unwrap()
}
fn owner_json(method: &str, path: &str, body: Value) -> Value {
    let serving = SERVING.lock().unwrap();
    let response = serving
        .as_ref()
        .unwrap()
        .serving
        .request_owner(method, path, &body.to_string())
        .unwrap();
    assert_eq!(response.status, 200, "{path}: {}", response.body);
    serde_json::from_str(&response.body).unwrap()
}

#[test]
fn native_folder_save_preview_start_replay_and_read_keep_real_custody() {
    if isolated("native_folder_save_preview_start_replay_and_read_keep_real_custody") {
        return;
    }
    let fixture = Fixture::new();
    let result = copy(&fixture.cache, &fixture.request(), None, None);
    assert_eq!(result["state"], "saved", "{result}");
    assert_eq!(result["folder_name"], "Picked notes");
    let id = result["import_id"].as_str().unwrap();
    let directory = fixture.root.join("imports").join(id);
    assert_eq!(
        fs::read(directory.join("tree/note.md")).unwrap(),
        b"# Picked notes\n\ncobalt iOS retained folder\n"
    );
    assert!(directory.join("tree/empty").is_dir());
    let metadata = solstone_core_import::read_import_metadata(&fixture.root, id).unwrap();
    assert_eq!(metadata["client_item_id"], "ios-native-folder");
    assert!(solstone_core_import::get_attempt_facts(&metadata).is_none());
    fixture.empty_cache();
    let mut start = json!({"path":result["value"]["path"],"timestamp":result["value"]["source_timestamp"],"import_id":id});
    let preview = owner_json("POST", "/app/import/api/preview", start.clone());
    assert!(preview["preview_token"].is_string(), "{preview}");
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
    assert_eq!(
        owner_json("POST", "/app/import/api/start", start)["replay"],
        true
    );
    let read = owner_json(
        "GET",
        "/app/transcripts/api/read/20261014?transcripts=1",
        Value::Null,
    );
    assert!(
        read["markdown"]
            .as_str()
            .unwrap()
            .contains("cobalt iOS retained folder"),
        "{read}"
    );
    fixture.finish();
}

#[test]
fn registration_failure_retains_admitted_identity_and_original_bytes() {
    if isolated("registration_failure_retains_admitted_identity_and_original_bytes") {
        return;
    }
    let fixture = Fixture::new();
    let result = copy(
        &fixture.cache,
        &fixture.request(),
        None,
        Some(fixture.root.join("config/journal.json")),
    );
    assert_eq!(result["state"], "registration_failed", "{result}");
    assert_eq!(result["folder_name"], "Picked notes");
    assert_eq!(result["custody"], "saved");
    let directory = fixture
        .root
        .join("imports")
        .join(result["import_id"].as_str().unwrap());
    assert_eq!(
        fs::read(directory.join("tree/note.md")).unwrap(),
        b"# Picked notes\n\ncobalt iOS retained folder\n"
    );
    assert!(directory.join("folder-custody.json").is_file());
    assert!(!directory.join("import.json").exists());
    fixture.empty_cache();
    fixture.finish();
}

#[test]
fn original_stop_holds_provider_and_cleanup_until_return_then_releases_root_use() {
    if isolated("original_stop_holds_provider_and_cleanup_until_return_then_releases_root_use") {
        return;
    }
    let fixture = Fixture::new();
    let cache = fixture.cache.clone();
    let request = fixture.request();
    let (entered, observed) = mpsc::channel();
    let (resume, held) = mpsc::channel();
    let worker = std::thread::spawn(move || copy(&cache, &request, Some((entered, held)), None));
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    journal_ios_processing_request_stop();
    solstone_core_journal_io::whole_journal_restore::hold_target_for_restore(&fixture.root);
    let path = fixture.root.clone();
    let (done, waited) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        solstone_core_journal_io::whole_journal_restore::wait_for_target_use_to_end(&path);
        done.send(()).unwrap();
    });
    let root = fixture.root.clone();
    // The real owner service retains this same root too. Retire it through its
    // actual drain while the provider still owns the original preparation.
    let (retired, retirement) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        fixture.finish();
        retired.send(()).unwrap();
        fixture
    });
    assert!(matches!(
        waited.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    resume.send(()).unwrap();
    assert_eq!(worker.join().unwrap()["state"], "refused");
    retirement.recv_timeout(Duration::from_secs(10)).unwrap();
    let fixture = owner.join().unwrap();
    waited.recv_timeout(Duration::from_secs(10)).unwrap();
    drain.join().unwrap();
    solstone_core_journal_io::whole_journal_restore::release_target_for_restore(&root);
    fixture.empty_cache();
    assert!(!fixture.root.join("imports").exists());
}
