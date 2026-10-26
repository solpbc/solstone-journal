// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_import::{SyncState, begin_folder, public_connection};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

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
        assert_eq!(request.backend, BackendName::Obsidian);
        assert_eq!(request.access_material, "opaque-native-grant");
        assert!(!request.source_identity.is_empty());
        assert!(!request.stop.is_requested());
        fs::create_dir(destination.join("empty")).unwrap();
        let note = destination.join("note.md");
        fs::write(&note, b"# Source note\n\ncobalt configured source\n").unwrap();
        let modified: chrono::DateTime<chrono::Utc> = "2026-10-14T09:00:00Z".parse().unwrap();
        fs::File::open(note)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(
                std::time::SystemTime::UNIX_EPOCH
                    + Duration::from_secs(modified.timestamp().try_into().unwrap()),
            ))
            .unwrap();
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
    ["--sync", "obsidian", "--save"].map(str::to_owned).to_vec()
}
fn configure(root: &solstone_core_journal_io::JournalRoot) -> String {
    let mut state = SyncState::empty(BackendName::Obsidian);
    let id = begin_folder(&mut state, "Notes", true, Some("opaque-native-grant")).unwrap();
    state
        .root_mut()
        .insert("extra".into(), serde_json::json!({"future":"kept"}));
    state.files_mut().insert(
        "prior-note.md".into(),
        serde_json::json!({"unknown":"kept"}),
    );
    write_sync_state_bound(root, &state, &WorkStop::new()).unwrap();
    id
}
fn state(root: &solstone_core_journal_io::JournalRoot) -> SyncState {
    let SyncStateRead::Loaded(state) = read_sync_state_bound(root, BackendName::Obsidian) else {
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
    let library = cache.join("pdfium-not-used-by-notes.so");
    fs::write(&library, b"notes never load PDFium").unwrap();
    let sources = Arc::new(ConfiguredSources::new(cache.to_path_buf(), provider));
    super::super::tests::fixture(packaged_import_factory_with_sources(library, sources))
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
    let read = server
        .request_owner(
            "GET",
            "/app/transcripts/api/read/20261014?transcripts=1",
            "",
        )
        .unwrap();
    assert_eq!(read.status, 200);
    assert!(
        read.body.contains("cobalt configured source"),
        "{}",
        read.body
    );
    assert!(
        root.path()
            .join("chronicle/20261014/import.obsidian")
            .is_dir()
    );
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
    let (root, server, host) = setup(cache.path(), provider);
    configure(&server.root);
    let stop = WorkStop::new();
    let request = host
        .import_request(args(), stop.clone(), Default::default())
        .unwrap();
    let worker = std::thread::spawn(move || assert!(request.prepare().is_err()));
    observed.recv_timeout(Duration::from_secs(10)).unwrap();
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
    assert!(
        !root
            .path()
            .join("chronicle/20261014/import.obsidian")
            .exists()
    );
    host.shutdown().unwrap();
    server.shutdown_for_retirement().unwrap();
}
