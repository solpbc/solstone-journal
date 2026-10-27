// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::foreground::{ForegroundProcessingHost, PreparedImport, TEST_LOCK};
use solstone_core_backup_web::BackupWebDeps;
use solstone_core_import::{WorkStop, read_import_metadata, validate_timestamp};
use solstone_core_journal_io::JournalRoot;
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> (
    tempfile::TempDir,
    ForegroundProcessingHost,
    Arc<AtomicUsize>,
) {
    let root = tempfile::tempdir().unwrap();
    let payload = root.path().join("payload");
    for relative in [
        "solstone/talent",
        "solstone/apps",
        "solstone/think/templates",
    ] {
        fs::create_dir_all(payload.join(relative)).unwrap();
    }
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(
        root.path().join("config/journal.json"),
        r#"{"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#,
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let factory: Arc<crate::foreground::ImportSeamFactory> = Arc::new(move |context| {
        observed.fetch_add(1, Ordering::AcqRel);
        let seams = context.seams_with_pdf(Box::new(solstone_core_import_sources::NullPdfWorker));
        Ok(PreparedImport::new(context.arguments().to_vec(), seams))
    });
    let host = ForegroundProcessingHost::open_with_imports(
        Arc::new(JournalRoot::open(root.path()).unwrap()),
        &payload,
        Arc::new(|| false),
        Arc::new(AtomicBool::new(false)),
        BackupWebDeps::new(root.path().to_path_buf()),
        factory,
        false,
        false,
    )
    .unwrap();
    host.stop_discovery_for_test();
    (root, host, calls)
}

fn stamp() -> Timestamp {
    validate_timestamp("20261026_123456").unwrap()
}

fn no_temporary_copy(cache: &Path) -> bool {
    fs::read_dir(cache.join("owner-bridge"))
        .unwrap()
        .next()
        .is_none()
}

#[test]
fn registration_failure_keeps_actual_custody_and_cleans_on_the_original_worker() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host, factory_calls) = fixture();
    let cache = tempfile::tempdir().unwrap();
    let worker = std::thread::current().id();
    let resource = Rc::new(()); // The provider resources need not be Send.
    let provider = Rc::clone(&resource);
    let registration = Rc::clone(&resource);
    let prep = host
        .import_preparation(WorkStop::new(), Default::default())
        .unwrap();
    let result = prep
        .with_folder_copy(
            cache.path(),
            "registration-error",
            &stamp(),
            |destination| {
                assert_eq!(std::thread::current().id(), worker);
                assert!(Rc::ptr_eq(&provider, &resource));
                fs::create_dir(destination.join("empty")).unwrap();
                fs::write(destination.join("note.md"), b"original admitted note").unwrap();
                Ok(())
            },
            |bound, admitted| -> Result<(), String> {
                assert_eq!(std::thread::current().id(), worker);
                assert!(Rc::ptr_eq(&registration, &resource));
                assert!(
                    bound
                        .canonical_path()
                        .join("imports")
                        .join(admitted.import_id.as_str())
                        .join("tree/note.md")
                        .exists()
                );
                Err("registration control".into())
            },
        )
        .unwrap();
    let FolderCopyResult::RegistrationFailed { folder, reason } = result else {
        panic!("committed save was reported as an ordinary refusal")
    };
    assert_eq!(reason, "registration control");
    let admitted = root.path().join("imports").join(folder.import_id.as_str());
    assert_eq!(
        fs::read(admitted.join("tree/note.md")).unwrap(),
        b"original admitted note"
    );
    assert!(admitted.join("tree/empty").is_dir());
    assert!(admitted.join("folder-custody.json").is_file());
    assert!(!admitted.join("import.json").exists());
    assert!(no_temporary_copy(cache.path()));
    assert_eq!(factory_calls.load(Ordering::Acquire), 0);
    host.shutdown().unwrap();
}

#[test]
fn successful_registration_and_stop_after_custody_preserve_committed_effects() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host, calls) = fixture();
    let cache = tempfile::tempdir().unwrap();
    let stop = WorkStop::new();
    let prep = host
        .import_preparation(stop.clone(), Default::default())
        .unwrap();
    let outcome = prep.with_folder_copy(cache.path(), "registered", &stamp(), |destination| {
        fs::write(destination.join("note.md"), b"kept after stop").unwrap();
        Ok(())
    }, |bound, folder| {
        stop.request();
        let metadata = serde_json::json!({"source":"obsidian", "file_path":format!("imports/{}/tree",folder.import_id.as_str()), "source_hash":folder.legacy_hash});
        folder.write_metadata_bound(bound, metadata.as_object().unwrap()).map(|()| bound.canonical_path().join("imports").join(folder.import_id.as_str()).join("import.json")).map_err(|error| error.to_string())
    }).unwrap();
    let FolderCopyResult::Registered { folder, value } = outcome else {
        panic!("registration failed")
    };
    assert!(value.is_file());
    assert_eq!(
        read_import_metadata(root.path(), folder.import_id.as_str()).unwrap()["source"],
        "obsidian"
    );
    assert_eq!(
        fs::read(
            root.path()
                .join("imports")
                .join(folder.import_id.as_str())
                .join("tree/note.md")
        )
        .unwrap(),
        b"kept after stop"
    );
    assert!(no_temporary_copy(cache.path()));
    assert_eq!(calls.load(Ordering::Acquire), 0);
    host.shutdown().unwrap();
}

#[test]
fn provider_error_removes_only_its_temporary_copy_without_admitting_an_import() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host, calls) = fixture();
    let cache = tempfile::tempdir().unwrap();
    let prep = host
        .import_preparation(WorkStop::new(), Default::default())
        .unwrap();
    let result = prep.with_folder_copy(
        cache.path(),
        "provider-error",
        &stamp(),
        |destination| {
            fs::write(destination.join("prefix.md"), b"partial provider read").unwrap();
            Err("provider control".into())
        },
        |_, _| -> Result<(), String> { panic!("failed provider reached registration") },
    );
    assert!(matches!(result, Err(FolderCopyError::Refused(_))));
    assert!(no_temporary_copy(cache.path()));
    assert!(!root.path().join("imports").exists());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    host.shutdown().unwrap();
}

#[test]
fn disconnected_cancelled_provider_retires_before_recovery_then_owner_resumes() {
    use solstone_core_convey_shell::recovery_drain::{
        register_recovery_hooks, relinquish_admitted_work, resume_after_whole_journal_restore,
    };
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host, calls) = fixture();
    let cache = tempfile::tempdir().unwrap();
    let cache_path = cache.path().to_path_buf();
    let stop = WorkStop::new();
    let prep = host
        .import_preparation(stop.clone(), Default::default())
        .unwrap();
    let (copied_send, copied) = mpsc::channel();
    let (return_send, provider_return) = mpsc::channel();
    let (outcome_send, outcome) = mpsc::channel();
    drop(outcome); // Native delivery is gone, but the service worker remains joined.
    let worker = std::thread::spawn(move || {
        let worker_id = std::thread::current().id();
        let resource = Rc::new(());
        let result = prep.with_folder_copy(
            &cache_path,
            "disconnected",
            &stamp(),
            |destination| {
                assert_eq!(Rc::strong_count(&resource), 1);
                fs::write(destination.join("prefix.md"), b"cancelled provider prefix").unwrap();
                copied_send.send(destination.to_path_buf()).unwrap();
                provider_return
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                assert_eq!(std::thread::current().id(), worker_id);
                assert!(!solstone_core_import::work_continues());
                Ok(())
            },
            |_, _| -> Result<(), String> { panic!("cancelled provider reached registration") },
        );
        assert!(matches!(result, Err(FolderCopyError::Refused(_))));
        assert!(no_temporary_copy(&cache_path));
        assert!(outcome_send.send(result).is_err());
    });
    let temporary = copied.recv_timeout(Duration::from_secs(5)).unwrap();
    stop.request();
    let (paused_send, paused) = mpsc::channel();
    let hooks = register_recovery_hooks(
        "fc2-folder-provider-drain",
        Arc::clone(&host.root),
        move |_| {
            paused_send.send(()).unwrap();
            Ok(())
        },
        |_| Ok(()),
    );
    let target = root.path().to_path_buf();
    let (drained_send, drained) = mpsc::channel();
    let drain = std::thread::spawn(move || {
        drained_send
            .send(relinquish_admitted_work(&target))
            .unwrap();
    });
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(drained.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(temporary.join("prefix.md").is_file());
    return_send.send(()).unwrap();
    worker.join().unwrap();
    drained
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    drain.join().unwrap();
    assert!(!temporary.exists());
    assert!(!root.path().join("imports").exists());
    resume_after_whole_journal_restore(root.path());
    drop(
        host.import_preparation(WorkStop::new(), Default::default())
            .unwrap(),
    );
    drop(hooks);
    assert_eq!(calls.load(Ordering::Acquire), 0);
    host.shutdown().unwrap();
}

#[test]
fn folder_timestamp_reads_bound_owner_zone_and_missing_config_without_admission() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host, calls) = fixture();
    let prep = host
        .import_preparation(WorkStop::new(), Default::default())
        .unwrap();
    fs::write(
        root.path().join("config/journal.json"),
        r#"{"identity":{"timezone":"Pacific/Auckland"},"processing":{"history_confirmed":true}}"#,
    )
    .unwrap();
    let zone = solstone_core_journal_config::parse_zone("Pacific/Auckland").unwrap();
    let before = chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d_%H%M%S")
        .to_string();
    let timestamp = prep.folder_timestamp().unwrap();
    let after = chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d_%H%M%S")
        .to_string();
    assert!(timestamp.as_str() >= before.as_str() && timestamp.as_str() <= after.as_str());
    fs::remove_file(root.path().join("config/journal.json")).unwrap();
    let zone = solstone_core_journal_config::host_zone();
    let before = chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d_%H%M%S")
        .to_string();
    let timestamp = prep.folder_timestamp().unwrap();
    let after = chrono::Utc::now()
        .with_timezone(&zone)
        .format("%Y%m%d_%H%M%S")
        .to_string();
    assert!(timestamp.as_str() >= before.as_str() && timestamp.as_str() <= after.as_str());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(!root.path().join("imports").exists());
    drop(prep);
    host.shutdown().unwrap();
}

#[test]
fn folder_timestamp_refuses_full_stop_and_replacement_without_using_foreign_config() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (root, host, calls) = fixture();
    let stop = WorkStop::new();
    let prep = host
        .import_preparation(stop.clone(), Default::default())
        .unwrap();
    stop.request();
    assert!(prep.folder_timestamp().is_err());
    drop(prep);
    let prep = host
        .import_preparation(WorkStop::new(), Default::default())
        .unwrap();
    let moved = root.path().with_extension("original-owner");
    fs::rename(root.path(), &moved).unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    let sentinel = b"{replacement-config-sentinel";
    fs::write(root.path().join("config/journal.json"), sentinel).unwrap();
    assert!(prep.folder_timestamp().is_err());
    assert_eq!(
        fs::read(root.path().join("config/journal.json")).unwrap(),
        sentinel
    );
    assert!(!moved.join("imports").exists());
    assert_eq!(calls.load(Ordering::Acquire), 0);
    drop(prep);
    host.request_stop();
    host.shutdown().unwrap();
    fs::remove_dir_all(moved).unwrap();
}
