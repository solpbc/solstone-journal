// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_import::{SyncState, begin_folder, public_connection};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// A recording the catalogue already holds as imported, so a sync finds it and has no work.
const CLIP: &[u8] = b"already imported clip";

fn clip_entry() -> serde_json::Value {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clip.wav");
    fs::write(&path, CLIP).unwrap();
    let hash = solstone_core_import::hash_source(&path)
        .unwrap()
        .into_inner();
    serde_json::json!({"status":"imported","hash":hash})
}

struct Provider {
    calls: AtomicUsize,
    lost: bool,
    entered: Option<mpsc::Sender<()>>,
    release: Option<std::sync::Mutex<mpsc::Receiver<()>>>,
}
impl SavedFolderProvider for Provider {
    fn copy(
        &self,
        request: SavedFolderRequest<'_>,
        destination: &Path,
    ) -> Result<(), SourceCopyError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        assert_eq!(request.backend, BackendName::Audio);
        assert_eq!(request.access_material, "opaque-native-grant");
        assert!(!request.source_identity.is_empty());
        assert!(!request.stop.is_requested());
        fs::create_dir(destination.join("empty")).unwrap();
        fs::write(destination.join("clip.wav"), CLIP).unwrap();
        if let Some(entered) = &self.entered {
            entered.send(()).unwrap();
            self.release
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            assert!(request.stop.is_requested());
        }
        if self.lost {
            Err(SourceCopyError::AccessLost)
        } else {
            Ok(())
        }
    }
}
fn provider(lost: bool) -> Arc<Provider> {
    Arc::new(Provider {
        calls: AtomicUsize::new(0),
        lost,
        entered: None,
        release: None,
    })
}
fn isolated(name: &str) -> bool {
    super::super::tests::in_isolated_child_exact(&format!("import_route::sources::tests::{name}"))
}
fn args() -> Vec<String> {
    ["--sync", "audio", "--save"].map(str::to_owned).to_vec()
}
fn configure(root: &solstone_core_journal_io::JournalRoot) -> String {
    let mut state = SyncState::empty(BackendName::Audio);
    let id = begin_folder(&mut state, "Notes", true, Some("opaque-native-grant")).unwrap();
    state
        .root_mut()
        .insert("extra".into(), serde_json::json!({"future":"kept"}));
    state.files_mut().insert(
        "prior-note.md".into(),
        serde_json::json!({"unknown":"kept"}),
    );
    state.files_mut().insert("clip.wav".into(), clip_entry());
    write_sync_state_bound(root, &state, &WorkStop::new()).unwrap();
    id
}
fn state(root: &solstone_core_journal_io::JournalRoot) -> SyncState {
    let SyncStateRead::Loaded(state) = read_sync_state_bound(root, BackendName::Audio) else {
        panic!("state unavailable")
    };
    state
}
fn cache_empty(cache: &Path) {
    assert!(
        fs::read_dir(cache.join("owner-bridge"))
            .unwrap()
            .next()
            .is_none()
    );
}
fn setup(
    cache: &Path,
    provider: Arc<Provider>,
) -> (
    tempfile::TempDir,
    Arc<crate::MobileServing>,
    Arc<crate::foreground::ForegroundProcessingHost>,
) {
    let (root, server, host, _) = setup_with_sources(cache, provider);
    (root, server, host)
}
fn setup_with_sources(
    cache: &Path,
    provider: Arc<Provider>,
) -> (
    tempfile::TempDir,
    Arc<crate::MobileServing>,
    Arc<crate::foreground::ForegroundProcessingHost>,
    Arc<ConfiguredSources>,
) {
    let library = cache.join("pdfium-not-used-by-notes.so");
    fs::write(&library, b"notes never load PDFium").unwrap();
    let sources = Arc::new(ConfiguredSources::new(cache.to_path_buf(), provider));
    let (root, server, host) = super::super::tests::fixture(packaged_import_factory_with_sources(
        library,
        Arc::clone(&sources),
    ));
    (root, server, host, sources)
}

#[test]
fn saved_source_factory_resolves_canonical_sync_and_preserves_its_catalogue() {
    if isolated("saved_source_factory_resolves_canonical_sync_and_preserves_its_catalogue") {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let provider = provider(false);
    let (root, server, host) = setup(cache.path(), Arc::clone(&provider));
    let identity = configure(&server.root);
    let mut argv = args();
    argv.push("--".into());
    let request = host
        .import_request(argv, WorkStop::new(), Default::default())
        .unwrap();
    let (run, _) = request.prepare().unwrap().run().unwrap();
    assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    cache_empty(cache.path());
    let stored = state(&server.root);
    let public = public_connection(&stored).unwrap();
    assert_eq!(public.source_identity, identity);
    assert_eq!(stored.root()["extra"], serde_json::json!({"future":"kept"}));
    assert_eq!(stored.root()["files"]["prior-note.md"]["unknown"], "kept");
    assert_eq!(stored.root()["files"]["prior-note.md"]["status"], "removed");
    assert_eq!(stored.root()["files"]["clip.wav"]["status"], "imported");
    assert!(root.path().join("imports/audio.json").is_file());
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn lost_native_grant_preserves_identity_and_catalogue_and_cleans_partial_copy() {
    if isolated("lost_native_grant_preserves_identity_and_catalogue_and_cleans_partial_copy") {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let provider = provider(true);
    let (_root, server, host) = setup(cache.path(), Arc::clone(&provider));
    let identity = configure(&server.root);
    let error = host
        .import_request(args(), WorkStop::new(), Default::default())
        .unwrap()
        .prepare()
        .err()
        .unwrap();
    assert_eq!(error, "source connection is unavailable");
    let stored = state(&server.root);
    let public = public_connection(&stored).unwrap();
    assert_eq!(public.source_identity, identity);
    assert_eq!(public.access_state, solstone_core_import::AccessState::Lost);
    assert!(stored.root().get("access_material").is_none());
    assert_eq!(
        stored.root()["files"]["prior-note.md"],
        serde_json::json!({"unknown":"kept"})
    );
    assert_eq!(stored.root()["extra"], serde_json::json!({"future":"kept"}));
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    cache_empty(cache.path());
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn source_stop_holds_original_use_until_provider_return_and_cleanup() {
    if isolated("source_stop_holds_original_use_until_provider_return_and_cleanup") {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let (entered, observed) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let provider = Arc::new(Provider {
        calls: AtomicUsize::new(0),
        lost: false,
        entered: Some(entered),
        release: Some(std::sync::Mutex::new(held)),
    });
    let (root, server, host, sources) = setup_with_sources(cache.path(), provider);
    let identity = configure(&server.root);
    let stop = WorkStop::new();
    let request = host
        .import_request(args(), stop.clone(), Default::default())
        .unwrap();
    let worker = std::thread::spawn(move || assert!(request.prepare().is_err()));
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
    let before = fs::read(root.path().join("imports/audio.json")).unwrap();
    assert!(matches!(
        sources.connection_guard(BackendName::Audio),
        Err(SourceConnectionError::Busy)
    ));
    assert_eq!(
        fs::read(root.path().join("imports/audio.json")).unwrap(),
        before
    );
    stop.request();
    solstone_core_journal_io::whole_journal_restore::hold_target_for_restore(root.path());
    let path = root.path().to_path_buf();
    let (done, waited) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        solstone_core_journal_io::whole_journal_restore::wait_for_target_use_to_end(&path);
        done.send(()).unwrap();
    });
    assert!(matches!(
        waited.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    worker.join().unwrap();
    waited.recv_timeout(Duration::from_secs(10)).unwrap();
    drain.join().unwrap();
    cache_empty(cache.path());
    solstone_core_journal_io::whole_journal_restore::release_target_for_restore(root.path());
    let mut guard = sources.connection_guard(BackendName::Audio).unwrap();
    let public = guard
        .update(
            &server.root,
            &WorkStop::new(),
            FolderConnectionChange::Forget {
                expected_identity: identity,
            },
        )
        .unwrap();
    assert!(!public.remembered);
    drop(guard);
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn connection_changes_share_sync_authority_and_keep_catalogues_and_grants_private() {
    if isolated("connection_changes_share_sync_authority_and_keep_catalogues_and_grants_private") {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    let provider = provider(false);
    let (_root, server, host, sources) = setup_with_sources(cache.path(), Arc::clone(&provider));
    let stop = WorkStop::new();
    let mut guard = sources.connection_guard(BackendName::Audio).unwrap();
    let first = guard
        .update(
            &server.root,
            &stop,
            FolderConnectionChange::Begin {
                folder_name: "Notes".into(),
                access_material: "opaque-native-grant".into(),
            },
        )
        .unwrap();
    assert!(!format!("{first:?}").contains("opaque-native-grant"));
    assert!(
        host.import_request(args(), WorkStop::new(), Default::default())
            .unwrap()
            .prepare()
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::Acquire), 0);
    drop(guard);
    let mut seeded = state(&server.root);
    seeded.files_mut().insert("clip.wav".into(), clip_entry());
    write_sync_state_bound(&server.root, &seeded, &WorkStop::new()).unwrap();
    let (run, _) = host
        .import_request(args(), WorkStop::new(), Default::default())
        .unwrap()
        .prepare()
        .unwrap()
        .run()
        .unwrap();
    assert_eq!(run.exit_code, 0, "{}{}", run.stdout, run.stderr);
    let catalogue = state(&server.root).root()["files"].clone();
    assert!(!catalogue.as_object().unwrap().is_empty());
    let mut guard = sources.connection_guard(BackendName::Audio).unwrap();
    let before = fs::read(server.root.canonical_path().join("imports/audio.json")).unwrap();
    assert_eq!(
        guard.update(
            &server.root,
            &stop,
            FolderConnectionChange::Forget {
                expected_identity: "other-card".into()
            }
        ),
        Err(SourceConnectionError::Refused)
    );
    assert_eq!(
        fs::read(server.root.canonical_path().join("imports/audio.json")).unwrap(),
        before
    );
    let refreshed = guard
        .update(
            &server.root,
            &stop,
            FolderConnectionChange::Reauthorize {
                expected_identity: first.source_identity.clone(),
                folder_name: "Notes".into(),
                access_material: "fresh-private-grant".into(),
            },
        )
        .unwrap();
    assert_eq!(refreshed.source_identity, first.source_identity);
    assert_eq!(state(&server.root).root()["files"], catalogue);
    assert!(!format!("{refreshed:?}").contains("fresh-private-grant"));
    let replaced = guard
        .update(
            &server.root,
            &stop,
            FolderConnectionChange::Replace {
                expected_identity: first.source_identity.clone(),
                folder_name: "Other Notes".into(),
                access_material: "other-private-grant".into(),
            },
        )
        .unwrap();
    assert_ne!(replaced.source_identity, first.source_identity);
    let replaced_state = state(&server.root);
    assert!(
        replaced_state.root()["files"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        replaced_state.root()["retained_catalogs"][&first.source_identity],
        catalogue
    );
    let forgotten = guard
        .update(
            &server.root,
            &stop,
            FolderConnectionChange::Forget {
                expected_identity: replaced.source_identity.clone(),
            },
        )
        .unwrap();
    assert_eq!(forgotten.source_identity, replaced.source_identity);
    assert!(!forgotten.remembered);
    assert_eq!(
        forgotten.access_state,
        solstone_core_import::AccessState::Absent
    );
    assert!(state(&server.root).root().get("access_material").is_none());
    assert!(matches!(
        sources.connection_guard(BackendName::Oura),
        Err(SourceConnectionError::Refused)
    ));
    assert!(matches!(
        sources.connection_guard(BackendName::Plaud),
        Err(SourceConnectionError::Refused)
    ));
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}

#[test]
fn connection_mutation_refuses_unreadable_state_stop_and_replacement_root_without_writes() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("journal");
    fs::create_dir(&path).unwrap();
    let root = solstone_core_journal_io::JournalRoot::open(&path).unwrap();
    fs::create_dir(path.join("imports")).unwrap();
    let record = path.join("imports/audio.json");
    fs::write(&record, b"{unreadable-state").unwrap();
    let sources = ConfiguredSources::new(parent.path().join("cache"), provider(false));
    let mut guard = sources.connection_guard(BackendName::Audio).unwrap();
    let change = || FolderConnectionChange::Begin {
        folder_name: "Notes".into(),
        access_material: "private-grant".into(),
    };
    assert_eq!(
        guard.update(&root, &WorkStop::new(), change()),
        Err(SourceConnectionError::Refused)
    );
    assert_eq!(fs::read(&record).unwrap(), b"{unreadable-state");
    fs::remove_file(&record).unwrap();
    let stopped = WorkStop::new();
    stopped.request();
    assert_eq!(
        guard.update(&root, &stopped, change()),
        Err(SourceConnectionError::Refused)
    );
    assert!(!record.exists());
    fs::rename(&path, parent.path().join("original")).unwrap();
    fs::create_dir_all(path.join("imports")).unwrap();
    fs::write(&record, b"replacement-sentinel").unwrap();
    assert_eq!(
        guard.update(&root, &WorkStop::new(), change()),
        Err(SourceConnectionError::Refused)
    );
    assert_eq!(fs::read(&record).unwrap(), b"replacement-sentinel");
    assert!(!parent.path().join("original/imports/audio.json").exists());
}
