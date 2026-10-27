// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::owner_folder::tests::{Fixture, isolated_full};
use serde_json::{Value, json};
use solstone_core_import::{WorkStop, installed_stop};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Default)]
struct Counts {
    calls: AtomicUsize,
    released: AtomicUsize,
}
struct Context {
    name: &'static str,
    main: std::thread::ThreadId,
    counts: Arc<Counts>,
    block: Option<(mpsc::Sender<()>, Mutex<mpsc::Receiver<()>>)>,
}

unsafe extern "C" fn copy(
    context: *mut c_void,
    identity: *const c_char,
    material: *const c_char,
    destination: *const c_char,
) -> i32 {
    let context = unsafe { &*context.cast::<Context>() };
    assert_ne!(std::thread::current().id(), context.main);
    assert!(installed_stop().is_some());
    assert!(crate::owner_folder::journal_owner_folder_copy_continues());
    assert!(!unsafe { CStr::from_ptr(identity) }.to_bytes().is_empty());
    assert_eq!(
        unsafe { CStr::from_ptr(material) }.to_str().unwrap(),
        "opaque-native-grant"
    );
    context.counts.calls.fetch_add(1, Ordering::SeqCst);
    let destination = Path::new(unsafe { CStr::from_ptr(destination) }.to_str().unwrap());
    let note = destination.join("note.md");
    fs::write(
        &note,
        format!(
            "# Native source\n\ncobalt {} retained source\n",
            context.name
        ),
    )
    .unwrap();
    let modified: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse().unwrap();
    fs::File::open(note)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(
            std::time::SystemTime::UNIX_EPOCH
                + Duration::from_secs(modified.timestamp().try_into().unwrap()),
        ))
        .unwrap();
    if let Some((entered, release)) = &context.block {
        entered.send(()).unwrap();
        release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(!crate::owner_folder::journal_owner_folder_copy_continues());
    }
    // A blocked provider deliberately returns late success; the native full
    // Stop and shared factory must still prevent native admission/publication.
    0
}
unsafe extern "C" fn release(context: *mut c_void) {
    let context = unsafe { Box::from_raw(context.cast::<Context>()) };
    context.counts.released.fetch_add(1, Ordering::SeqCst);
}
fn install(
    cache: &Path,
    name: &'static str,
    block: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
) -> (u64, Arc<Counts>) {
    let counts = Arc::new(Counts::default());
    let context = Box::new(Context {
        name,
        main: std::thread::current().id(),
        counts: Arc::clone(&counts),
        block: block.map(|(entered, release)| (entered, Mutex::new(release))),
    });
    let cache = CString::new(cache.to_str().unwrap()).unwrap();
    let identity = unsafe {
        journal_owner_sources_install(
            cache.as_ptr(),
            Box::into_raw(context).cast(),
            Some(copy),
            Some(release),
        )
    };
    assert_ne!(identity, 0);
    (identity, counts)
}
fn cache() -> PathBuf {
    let path = std::env::temp_dir().join(format!("fc2-native-sources-{}", std::process::id()));
    fs::create_dir(&path).unwrap();
    path
}

#[derive(Default)]
struct Permissions {
    grants: Mutex<std::collections::BTreeSet<String>>,
    acquired: AtomicUsize,
    released: AtomicUsize,
    scope_seen: AtomicUsize,
    block: Option<(mpsc::Sender<()>, Mutex<mpsc::Receiver<()>>)>,
}

unsafe extern "C" fn permission(context: *mut c_void, action: i32, material: *const c_char) -> i32 {
    let context = unsafe { &*context.cast::<Permissions>() };
    if installed_stop().is_some() {
        context.scope_seen.fetch_add(1, Ordering::SeqCst);
    }
    let Ok(material) = (unsafe { CStr::from_ptr(material) }).to_str() else {
        return -1;
    };
    match action {
        0 => {
            if !crate::owner_folder::journal_owner_folder_copy_continues() {
                return -1;
            }
            let new = context.grants.lock().unwrap().insert(material.into());
            context.acquired.fetch_add(1, Ordering::SeqCst);
            if let Some((entered, release)) = &context.block
                && (entered.send(()).is_err()
                    || release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .is_err())
            {
                return -1;
            }
            if crate::owner_folder::journal_owner_folder_copy_continues() {
                i32::from(new)
            } else {
                -1
            }
        }
        1 | 2 => {
            context.grants.lock().unwrap().remove(material);
            context.released.fetch_add(1, Ordering::SeqCst);
            0
        }
        3 => i32::from(context.grants.lock().unwrap().contains(material)),
        _ => -1,
    }
}

fn connection_request(
    fixture: &Fixture,
    action: &str,
    backend: &str,
    material: &str,
    expected: Option<&str>,
) -> Value {
    let mut request = fixture.request();
    request["source"] = json!(backend);
    request["action"] = json!(action);
    request["access_material"] = json!(material);
    request["expected_identity"] = json!(expected);
    request
}

fn change_connection(request: Value, permissions: &Arc<Permissions>) -> Value {
    let context = Arc::as_ptr(permissions) as usize;
    std::thread::spawn(move || {
        let request = serde_json::to_vec(&request).unwrap();
        let pointer = unsafe {
            super::connection::journal_owner_source_change(
                request.as_ptr(),
                request.len(),
                context as *mut c_void,
                Some(permission),
            )
        };
        let reply = unsafe { CString::from_raw(pointer) };
        serde_json::from_slice(reply.as_bytes()).unwrap()
    })
    .join()
    .unwrap()
}

#[test]
fn native_connection_changes_keep_private_material_and_shared_grants_truthful() {
    if isolated_full(
        "saved_sources::tests::native_connection_changes_keep_private_material_and_shared_grants_truthful",
    ) {
        return;
    }
    let cache = cache();
    let (identity, _) = install(&cache, "configuration", None);
    let fixture = Fixture::new();
    let permissions = Arc::new(Permissions::default());
    let prior = serde_json::json!({"source_path":"retained-desktop-path","unknown":{"kept":true},"files":{"prior.md":{"status":"imported","extra":7}}});
    fs::create_dir_all(fixture.root.join("imports")).unwrap();
    fs::write(
        fixture.root.join("imports/obsidian.json"),
        serde_json::to_vec(&prior).unwrap(),
    )
    .unwrap();
    let first = "content://synthetic/tree/first";
    let second = "content://synthetic/tree/second";
    let begin = change_connection(
        connection_request(&fixture, "begin", "obsidian", first, None),
        &permissions,
    );
    assert_eq!(begin["state"], "updated", "{begin}");
    assert!(!begin.to_string().contains("content://"));
    let connected: Value =
        serde_json::from_slice(&fs::read(fixture.root.join("imports/obsidian.json")).unwrap())
            .unwrap();
    for key in ["files", "source_path", "unknown"] {
        assert_eq!(connected[key], prior[key]);
    }

    let original_id = begin["connection"]["source_identity"].as_str().unwrap();
    let audio = change_connection(
        connection_request(&fixture, "begin", "audio", first, None),
        &permissions,
    );
    assert_eq!(audio["state"], "updated");
    let audio_id = audio["connection"]["source_identity"].as_str().unwrap();
    let changed = change_connection(
        connection_request(&fixture, "replace", "obsidian", second, Some(original_id)),
        &permissions,
    );
    assert_eq!(changed["permissions"], "retained_for_another_source");
    assert_ne!(changed["connection"]["source_identity"], original_id);
    assert!(permissions.grants.lock().unwrap().contains(first));
    let forgotten = change_connection(
        connection_request(&fixture, "forget", "audio", "", Some(audio_id)),
        &permissions,
    );
    assert_eq!(forgotten["connection"]["remembered"], false);
    assert_eq!(forgotten["permissions"], "released");
    assert!(!permissions.grants.lock().unwrap().contains(first));
    assert!(permissions.grants.lock().unwrap().contains(second));
    let before = permissions.acquired.load(Ordering::SeqCst);
    let stale = change_connection(
        connection_request(
            &fixture,
            "reauthorize",
            "obsidian",
            first,
            Some(original_id),
        ),
        &permissions,
    );
    assert_eq!(stale["state"], "refused");
    assert_eq!(permissions.acquired.load(Ordering::SeqCst), before);
    let held = change_connection(
        connection_request(&fixture, "begin", "oura", first, None),
        &permissions,
    );
    assert_eq!(held["state"], "refused");
    assert_eq!(permissions.acquired.load(Ordering::SeqCst), before);
    fs::write(
        fixture.root.join("imports/obsidian.json"),
        b"unreadable saved state",
    )
    .unwrap();
    let unreadable = change_connection(
        connection_request(&fixture, "begin", "obsidian", first, None),
        &permissions,
    );
    assert_eq!(unreadable["state"], "refused");
    assert_eq!(permissions.acquired.load(Ordering::SeqCst), before);
    assert_eq!(
        fs::read(fixture.root.join("imports/obsidian.json")).unwrap(),
        b"unreadable saved state"
    );
    let partial = br#"{"source_path":"retained","folder_name":"partial","files":{"prior.md":{"status":"imported"}}}"#;
    fs::write(fixture.root.join("imports/obsidian.json"), partial).unwrap();
    let broken = change_connection(
        connection_request(&fixture, "begin", "obsidian", first, None),
        &permissions,
    );
    assert_eq!(broken["state"], "refused");
    assert_eq!(permissions.acquired.load(Ordering::SeqCst), before);
    assert_eq!(
        fs::read(fixture.root.join("imports/obsidian.json")).unwrap(),
        partial
    );
    assert!(permissions.scope_seen.load(Ordering::SeqCst) > 0);
    fixture.stop();
    fixture.stopped();
    journal_owner_sources_retire(identity);
    fs::remove_dir_all(cache).unwrap();
}

#[test]
fn native_connection_stop_drains_actual_permission_return_and_rollback_before_root_retirement() {
    if isolated_full(
        "saved_sources::tests::native_connection_stop_drains_actual_permission_return_and_rollback_before_root_retirement",
    ) {
        return;
    }
    let cache = cache();
    let (identity, _) = install(&cache, "configuration-stop", None);
    let fixture = Fixture::new();
    let (entered, observed) = mpsc::channel();
    let (resume, held) = mpsc::channel();
    let permissions = Arc::new(Permissions {
        block: Some((entered, Mutex::new(held))),
        ..Default::default()
    });
    let request = connection_request(
        &fixture,
        "begin",
        "obsidian",
        "content://synthetic/tree/stopped",
        None,
    );
    let retained = Arc::clone(&permissions);
    let worker = std::thread::spawn(move || change_connection(request, &retained));
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    fixture.stop();
    assert_eq!(
        crate::retirement::wait_for_process(Duration::from_millis(100)),
        crate::retirement::Outcome::Pending
    );
    assert_eq!(permissions.released.load(Ordering::SeqCst), 0);
    resume.send(()).unwrap();
    let reply = worker.join().unwrap();
    assert_eq!(reply["state"], "refused", "{reply}");
    assert!(permissions.grants.lock().unwrap().is_empty());
    assert!(permissions.released.load(Ordering::SeqCst) > 0);
    fixture.stopped();
    assert!(!fixture.root.join("imports/obsidian.json").exists());
    journal_owner_sources_retire(identity);
    fs::remove_dir_all(cache).unwrap();
}
fn configure() {
    let (root, sources, preparation) = {
        let serving = crate::SERVING.lock().unwrap();
        let owner = serving.as_ref().unwrap();
        (
            Arc::clone(&owner.root),
            Arc::clone(owner.sources.as_ref().unwrap()),
            owner
                .host
                .as_ref()
                .unwrap()
                .import_preparation(WorkStop::new(), Default::default())
                .unwrap(),
        )
    };
    let mut guard = sources.connection_guard(BackendName::Obsidian).unwrap();
    guard
        .update(
            &root,
            preparation.stop(),
            journal_mobile_serving::import_route::FolderConnectionChange::Begin {
                folder_name: "Notes".into(),
                access_material: "opaque-native-grant".into(),
            },
        )
        .unwrap();
    preparation.checkpoint().unwrap();
}
fn request() -> journal_mobile_serving::foreground::ImportRequest {
    crate::SERVING
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .host
        .as_ref()
        .unwrap()
        .import_request(
            ["--sync", "obsidian", "--save"].map(str::to_owned).to_vec(),
            WorkStop::new(),
            Default::default(),
        )
        .unwrap()
}
fn status() -> serde_json::Value {
    let request = serde_json::json!({"source":"obsidian","admission_generation":crate::THINKING_GENERATION.load(Ordering::SeqCst)}).to_string();
    let raw = unsafe { journal_owner_sources_status(request.as_ptr(), request.len()) };
    let text = unsafe { CString::from_raw(raw) };
    serde_json::from_slice(text.as_bytes()).unwrap()
}

#[test]
fn native_provider_registration_stays_with_its_host_and_produces_readable_source_output() {
    if isolated_full(
        "saved_sources::tests::native_provider_registration_stays_with_its_host_and_produces_readable_source_output",
    ) {
        return;
    }
    let cache = cache();
    let (first, alpha) = install(&cache, "alpha", None);
    let fixture = Fixture::new();
    configure();
    let info = status();
    assert_eq!(info["ok"], true, "{info}");
    assert_eq!(info["value"]["connection"]["remembered"], true);
    assert!(!info.to_string().contains("opaque-native-grant"));
    let (second, beta) = install(&cache, "beta", None);
    journal_owner_sources_retire(first);
    assert_eq!(
        INSTALLATION.lock().unwrap().as_ref().unwrap().identity,
        second
    );
    let request = request();
    let run = std::thread::spawn(move || request.prepare().unwrap().run().unwrap().0)
        .join()
        .unwrap();
    assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(alpha.calls.load(Ordering::SeqCst), 1);
    assert_eq!(beta.calls.load(Ordering::SeqCst), 0);
    let read = crate::SERVING
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .handle()
        .unwrap()
        .request_owner(
            "GET",
            "/app/transcripts/api/read/20261014?transcripts=1",
            "",
        )
        .unwrap();
    assert_eq!(read.status, 200);
    assert!(
        read.body.contains("cobalt alpha retained source"),
        "{}",
        read.body
    );
    assert!(!read.body.contains("cobalt beta retained source"));
    assert_eq!(alpha.released.load(Ordering::SeqCst), 0);
    fixture.stop();
    fixture.stopped();
    assert_eq!(alpha.released.load(Ordering::SeqCst), 1);
    journal_owner_sources_retire(second);
    assert_eq!(beta.released.load(Ordering::SeqCst), 1);
    fs::remove_dir_all(cache).unwrap();
}

#[test]
fn native_provider_stop_keeps_context_and_cleanup_until_actual_return() {
    if isolated_full(
        "saved_sources::tests::native_provider_stop_keeps_context_and_cleanup_until_actual_return",
    ) {
        return;
    }
    let cache = cache();
    let (entered, observed) = mpsc::channel();
    let (resume, held) = mpsc::channel();
    let (identity, counts) = install(&cache, "held", Some((entered, held)));
    let fixture = Fixture::new();
    configure();
    let request = request();
    let worker = std::thread::spawn(move || assert!(request.prepare().is_err()));
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    journal_owner_sources_retire(identity);
    assert_eq!(counts.released.load(Ordering::SeqCst), 0);
    fixture.stop();
    assert_eq!(
        crate::retirement::wait_for_process(Duration::from_millis(100)),
        crate::retirement::Outcome::Pending
    );
    resume.send(()).unwrap();
    worker.join().unwrap();
    fixture.stopped();
    assert_eq!(counts.released.load(Ordering::SeqCst), 1);
    assert!(
        fs::read_dir(cache.join("owner-bridge"))
            .unwrap()
            .next()
            .is_none()
    );
    assert!(
        !fixture
            .root
            .join("chronicle/20261014/import.obsidian")
            .exists()
    );
    fs::remove_dir_all(cache).unwrap();
}
