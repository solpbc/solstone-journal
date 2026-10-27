// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::owner_folder::tests::{Fixture, isolated_full};
use serde_json::{Value, json};
use solstone_core_import::WorkStop;
use std::ffi::c_void;
use std::fs;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

static ALPHA: AtomicUsize = AtomicUsize::new(0);
static BETA: AtomicUsize = AtomicUsize::new(0);
static COPY_BLOCK: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = Mutex::new(None);
const MATERIAL: &str = "c3ludGhldGljLWJvb2ttYXJr";

unsafe extern "C" fn copy_alpha(material: *const c_char, destination: *const c_char) -> i32 {
    if material.is_null()
        || destination.is_null()
        || !crate::owner_folder::journal_ios_owner_folder_copy_continues()
        || unsafe { CStr::from_ptr(material) }.to_bytes() != MATERIAL.as_bytes()
    {
        return -1;
    }
    ALPHA.fetch_add(1, Ordering::SeqCst);
    let Ok(destination) = (unsafe { CStr::from_ptr(destination) }).to_str() else {
        return -1;
    };
    let note = Path::new(destination).join("note.md");
    let copied = || -> Result<(), Box<dyn std::error::Error>> {
        fs::write(&note, b"# Saved folder\n\ncobalt iOS retained source\n")?;
        let modified: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse()?;
        fs::File::open(&note)?.set_times(fs::FileTimes::new().set_modified(
            std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(modified.timestamp().try_into()?),
        ))?;
        Ok(())
    };
    if copied().is_ok() { 0 } else { -1 }
}
unsafe extern "C" fn copy_beta(_: *const c_char, _: *const c_char) -> i32 {
    BETA.fetch_add(1, Ordering::SeqCst);
    -1
}
unsafe extern "C" fn copy_blocked(_: *const c_char, _: *const c_char) -> i32 {
    let (entered, release) = COPY_BLOCK.lock().unwrap().take().unwrap();
    entered.send(()).unwrap();
    release.recv_timeout(Duration::from_secs(10)).unwrap();
    // A provider's late success cannot override the original native Stop.
    assert!(!crate::owner_folder::journal_ios_owner_folder_copy_continues());
    0
}

struct Bookmark {
    worker: std::thread::ThreadId,
    local: Rc<()>,
    entered: Option<mpsc::Sender<()>>,
    release: Option<mpsc::Receiver<()>>,
    calls: Arc<AtomicUsize>,
}
unsafe extern "C" fn bookmark(context: *mut c_void, output: *mut u8, capacity: usize) -> isize {
    if context.is_null() || output.is_null() || capacity < MATERIAL.len() {
        return -1;
    }
    let value = unsafe { &*context.cast::<Bookmark>() };
    if std::thread::current().id() != value.worker
        || Rc::strong_count(&value.local) != 1
        || !crate::owner_folder::journal_ios_owner_folder_copy_continues()
    {
        return -1;
    }
    value.calls.fetch_add(1, Ordering::SeqCst);
    if let Some(entered) = &value.entered
        && entered.send(()).is_err()
    {
        return -1;
    }
    if let Some(release) = &value.release
        && release.recv_timeout(Duration::from_secs(10)).is_err()
    {
        return -1;
    }
    unsafe { std::ptr::copy_nonoverlapping(MATERIAL.as_ptr(), output, MATERIAL.len()) };
    MATERIAL.len() as isize
}

fn request(fixture: &Fixture, action: &str) -> Value {
    let mut value = fixture.request();
    value["action"] = json!(action);
    value
}
fn change(
    value: Value,
    entered: Option<mpsc::Sender<()>>,
    release: Option<mpsc::Receiver<()>>,
    calls: Arc<AtomicUsize>,
) -> Value {
    std::thread::spawn(move || {
        let context = Bookmark {
            worker: std::thread::current().id(),
            local: Rc::new(()),
            entered,
            release,
            calls,
        };
        let data = serde_json::to_vec(&value).unwrap();
        let pointer = unsafe {
            super::connection::journal_ios_owner_source_change(
                data.as_ptr(),
                data.len(),
                (&context as *const Bookmark).cast_mut().cast(),
                Some(bookmark),
            )
        };
        let reply = unsafe { CString::from_raw(pointer) };
        serde_json::from_slice(reply.as_bytes()).unwrap()
    })
    .join()
    .unwrap()
}
fn cache() -> PathBuf {
    let path = std::env::temp_dir().join(format!("fc2-ios-saved-source-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}
fn install(cache: &Path, callback: CopyProvider) -> u64 {
    let path = CString::new(cache.to_str().unwrap()).unwrap();
    unsafe { journal_ios_owner_sources_install(path.as_ptr(), Some(callback)) }
}

#[test]
fn saved_source_connection_uses_retained_provider_and_preserves_catalogue() {
    if isolated_full(
        "saved_sources::tests::saved_source_connection_uses_retained_provider_and_preserves_catalogue",
    ) {
        return;
    }
    let cache = cache();
    let identity = install(&cache, copy_alpha);
    assert_ne!(identity, 0);
    let fixture = Fixture::new();
    let prior = json!({"source_path":"retained-desktop-path","files":{},"unknown":{"kept":true}});
    fs::create_dir_all(fixture.root.join("imports")).unwrap();
    fs::write(
        fixture.root.join("imports/obsidian.json"),
        serde_json::to_vec(&prior).unwrap(),
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let result = change(request(&fixture, "begin"), None, None, Arc::clone(&calls));
    assert_eq!(result["state"], "updated", "{result}");
    assert!(!result.to_string().contains(MATERIAL));
    let state: Value =
        serde_json::from_slice(&fs::read(fixture.root.join("imports/obsidian.json")).unwrap())
            .unwrap();
    for key in ["source_path", "files", "unknown"] {
        assert_eq!(state[key], prior[key]);
    }
    assert_eq!(
        install(&cache, copy_alpha),
        identity,
        "duplicate startup preserves exact registration"
    );
    assert_eq!(
        install(&cache, copy_beta),
        0,
        "failed alternate startup cannot replace active owner"
    );
    journal_ios_owner_sources_retire(identity + 1);
    assert_eq!(
        INSTALLATION.lock().unwrap().as_ref().unwrap().identity,
        identity
    );
    let import = {
        let current = crate::SERVING.lock().unwrap();
        current
            .as_ref()
            .unwrap()
            .processing
            .as_ref()
            .unwrap()
            .import_request(
                ["--sync", "obsidian", "--save"].map(str::to_owned).to_vec(),
                WorkStop::new(),
                Default::default(),
            )
            .unwrap()
    };
    let run = std::thread::spawn(move || import.prepare().unwrap().run().unwrap().0)
        .join()
        .unwrap();
    assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(ALPHA.load(Ordering::SeqCst), 1);
    assert_eq!(BETA.load(Ordering::SeqCst), 0);
    let read = crate::SERVING
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .serving
        .request_owner(
            "GET",
            "/app/transcripts/api/read/20261014?transcripts=1",
            "",
        )
        .unwrap();
    assert_eq!(read.status, 200);
    assert!(
        read.body.contains("cobalt iOS retained source"),
        "{}",
        read.body
    );
    fixture.finish();
    assert!(INSTALLATION.lock().unwrap().is_none());
    fs::remove_dir_all(cache).unwrap();
}

#[test]
fn saved_source_stop_waits_for_bookmark_return_and_refuses_publication() {
    if isolated_full(
        "saved_sources::tests::saved_source_stop_waits_for_bookmark_return_and_refuses_publication",
    ) {
        return;
    }
    let cache = cache();
    assert_ne!(install(&cache, copy_alpha), 0);
    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let value = request(&fixture, "begin");
    let (entered, observed) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let worker = std::thread::spawn(move || change(value, Some(entered), Some(blocked), calls));
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    crate::journal_ios_processing_request_stop();
    let (finished, done) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        fixture.finish();
        finished.send(()).unwrap();
        fixture
    });
    assert!(
        done.recv_timeout(Duration::from_millis(100)).is_err(),
        "Stop cannot acknowledge while bookmark/native ownership remains"
    );
    release.send(()).unwrap();
    let reply = worker.join().unwrap();
    assert_eq!(reply["state"], "refused", "{reply}");
    done.recv_timeout(Duration::from_secs(10)).unwrap();
    let fixture = drain.join().unwrap();
    assert!(!root.join("imports/obsidian.json").exists());
    assert!(INSTALLATION.lock().unwrap().is_none());
    drop(fixture);
    fs::remove_dir_all(cache).unwrap();
}

#[test]
fn saved_source_partial_metadata_and_stale_page_refuse_before_bookmark() {
    if isolated_full(
        "saved_sources::tests::saved_source_partial_metadata_and_stale_page_refuse_before_bookmark",
    ) {
        return;
    }
    let cache = cache();
    assert_ne!(install(&cache, copy_alpha), 0);
    let fixture = Fixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let partial = br#"{"folder_name":"partial","files":{"owned.md":{"status":"imported"}}}"#;
    fs::create_dir_all(fixture.root.join("imports")).unwrap();
    fs::write(fixture.root.join("imports/obsidian.json"), partial).unwrap();
    let reply = change(request(&fixture, "begin"), None, None, Arc::clone(&calls));
    assert_eq!(reply["state"], "refused");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(fixture.root.join("imports/obsidian.json")).unwrap(),
        partial
    );
    fs::remove_file(fixture.root.join("imports/obsidian.json")).unwrap();
    let mut stale = request(&fixture, "begin");
    let retired = crate::owner_response::journal_ios_owner_authority_issue();
    crate::owner_response::journal_ios_owner_authority_retire(retired);
    stale["authority"] = json!(retired);
    assert_eq!(
        change(stale, None, None, Arc::clone(&calls))["state"],
        "refused"
    );
    let mut body = request(&fixture, "begin");
    body["source"] = json!("oura");
    assert_eq!(
        change(body, None, None, Arc::clone(&calls))["state"],
        "refused"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    fixture.finish();
    fs::remove_dir_all(cache).unwrap();
}

#[test]
fn saved_source_stop_retains_provider_until_copy_and_cleanup_join() {
    if isolated_full(
        "saved_sources::tests::saved_source_stop_retains_provider_until_copy_and_cleanup_join",
    ) {
        return;
    }
    let cache = cache();
    let identity = install(&cache, copy_blocked);
    assert_ne!(identity, 0);
    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let reply = change(
        request(&fixture, "begin"),
        None,
        None,
        Arc::new(AtomicUsize::new(0)),
    );
    assert_eq!(reply["state"], "updated", "{reply}");
    let (entered, observed) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    *COPY_BLOCK.lock().unwrap() = Some((entered, blocked));
    let import = crate::SERVING
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .processing
        .as_ref()
        .unwrap()
        .import_request(
            ["--sync", "obsidian", "--save"].map(str::to_owned).to_vec(),
            WorkStop::new(),
            Default::default(),
        )
        .unwrap();
    let worker = std::thread::spawn(move || {
        import
            .prepare()
            .and_then(|prepared| prepared.run().map_err(|error| error.to_string()))
    });
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(install(&cache, copy_beta), 0);
    crate::journal_ios_processing_request_stop();
    let (finished, done) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        fixture.finish();
        finished.send(()).unwrap();
        fixture
    });
    assert!(done.recv_timeout(Duration::from_millis(100)).is_err());
    assert_eq!(
        INSTALLATION.lock().unwrap().as_ref().unwrap().identity,
        identity
    );
    release.send(()).unwrap();
    assert!(
        worker.join().unwrap().is_err(),
        "late copy success must refuse"
    );
    done.recv_timeout(Duration::from_secs(10)).unwrap();
    let fixture = drain.join().unwrap();
    assert!(INSTALLATION.lock().unwrap().is_none());
    assert!(!root.join("20261014").exists());
    assert_eq!(
        fs::read_dir(cache.join("owner-bridge")).unwrap().count(),
        0,
        "copy staging retired before Stop acknowledgement"
    );
    drop(fixture);
    fs::remove_dir_all(cache).unwrap();
}
