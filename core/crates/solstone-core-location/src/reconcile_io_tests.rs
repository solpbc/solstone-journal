// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn place(name: &str) -> crate::places::Place {
    crate::places::Place {
        id: "owner-place".into(),
        latitude: 39.7555,
        longitude: -105.2211,
        name: Some(name.into()),
        alias: None,
        hidden: false,
    }
}

#[test]
fn place_update_keeps_its_locked_namespace_and_preserves_same_root_choices() {
    let journal = tempfile::tempdir().unwrap();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let scope = Scope::install(Arc::clone(&root)).unwrap();
    crate::places::PlaceStore::update(journal.path(), |store| {
        store.places.push(place("first owner name"))
    })
    .unwrap();
    let path = crate::places::store_path(journal.path());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    crate::places::PlaceStore::update(journal.path(), |store| {
        store.places[0].name = Some("kept owner name".into())
    })
    .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let before = fs::read(&path).unwrap();
    let directory = path.parent().unwrap();
    let displaced = directory.with_file_name("map-original");
    let replacement = br#"{"version":1,"places":[{"id":"replacement","latitude":1.0,"longitude":2.0,"name":"replacement owner choice"}]}"#;
    let result = crate::places::PlaceStore::update(journal.path(), |store| {
        fs::rename(directory, &displaced).unwrap();
        fs::create_dir(directory).unwrap();
        fs::write(&path, replacement).unwrap();
        store.places.clear();
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), replacement);
    assert_eq!(fs::read(displaced.join("places.json")).unwrap(), before);
    assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    fs::remove_file(&path).unwrap();
    fs::remove_dir(directory).unwrap();
    fs::rename(&displaced, directory).unwrap();
    assert!(
        crate::places::PlaceStore::update(journal.path(), |store| store.places.clear()).is_err(),
        "an operation cannot revive after observing substituted authority"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(scope);
    let _fresh = Scope::install(root).unwrap();
    crate::places::PlaceStore::update(journal.path(), |store| store.places[0].hidden = true)
        .unwrap();
    let saved = crate::places::PlaceStore::read_strict(journal.path()).unwrap();
    assert_eq!(saved.places[0].name.as_deref(), Some("kept owner name"));
    assert!(saved.places[0].hidden);
}

#[test]
fn admitted_place_update_leaves_a_replacement_root_empty() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    let displaced = temporary.path().join("original");
    fs::create_dir(&journal).unwrap();
    let root = Arc::new(JournalRoot::open(&journal).unwrap());
    let _scope = Scope::install(Arc::clone(&root)).unwrap();
    crate::places::PlaceStore::update(&journal, |store| {
        store.places.push(place("retained owner choice"));
    })
    .unwrap();
    let before = fs::read(crate::places::store_path(&journal)).unwrap();
    let result = crate::places::PlaceStore::update(&journal, |store| {
        fs::rename(&journal, &displaced).unwrap();
        fs::create_dir(&journal).unwrap();
        store.places.clear();
    });
    assert!(result.is_err());
    assert_eq!(fs::read_dir(&journal).unwrap().count(), 0);
    assert_eq!(
        fs::read(crate::places::store_path(&displaced)).unwrap(),
        before
    );
    let mut reconciler = crate::Reconciler::default();
    assert!(crate::reconcile_journal_owned(root, &mut reconciler).is_err());
    assert_eq!(
        fs::read_dir(&journal).unwrap().count(),
        0,
        "accuracy, places and admission locks must not create replacement entries"
    );
}

#[test]
fn namespace_discovered_substitution_refuses_later_transactions_in_the_same_frame() {
    let journal = tempfile::tempdir().unwrap();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let _scope = Scope::install(root).unwrap();
    crate::places::PlaceStore::update(journal.path(), |store| {
        store.places.push(place("kept owner choice"));
    })
    .unwrap();
    let path = crate::places::store_path(journal.path());
    let before = fs::read(&path).unwrap();
    let directory = path.parent().unwrap();
    let displaced = directory.with_file_name("map-original");
    transaction(|| {
        let owner = owner().unwrap();
        owner.check().unwrap();
        let namespace = owner.namespace.as_ref().unwrap();
        let relative = Path::new("solstone/apps/map");
        namespace.directory(relative, false).unwrap().unwrap();
        fs::rename(directory, &displaced).unwrap();
        fs::create_dir(directory).unwrap();
        // The namespace observes this change first, after Owner::check.
        assert!(namespace.directory(relative, false).is_err());
    });
    assert!(owner().unwrap().refused.load(Ordering::Acquire));
    assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
    fs::remove_dir(directory).unwrap();
    fs::rename(&displaced, directory).unwrap();
    assert!(
        crate::places::PlaceStore::update(journal.path(), |store| {
            store.places.clear();
        })
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn owned_place_update_refuses_corrupt_choices_without_replacing_them() {
    let journal = tempfile::tempdir().unwrap();
    let path = crate::places::store_path(journal.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{corrupt owner choices").unwrap();
    let _scope = Scope::install(Arc::new(JournalRoot::open(journal.path()).unwrap())).unwrap();
    assert!(
        crate::places::PlaceStore::update(journal.path(), |store| store.places.push(place("new")))
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"{corrupt owner choices");
}

#[test]
fn owned_pack_snapshot_preserves_verified_files_and_actual_shared_generation_hold() {
    use crate::accuracy::{INSTALLED_RECORD, Installed, InstalledFile, accuracy_root};
    use sha2::{Digest, Sha256};
    let journal = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/accuracy");
    let installed = Installed {
        version: "fixture-1".into(),
        folder: String::new(),
        cache_generation: None,
        files: ["venues.slpk", "containment.slpk"]
            .into_iter()
            .map(|name| {
                let bytes = fs::read(fixtures.join(name)).unwrap();
                InstalledFile {
                    file: name.into(),
                    sha256: format!("{:x}", Sha256::digest(&bytes)),
                    size_bytes: bytes.len() as u64,
                }
            })
            .collect(),
    };
    let directory = installed.directory(journal.path());
    fs::create_dir_all(&directory).unwrap();
    for file in &installed.files {
        fs::copy(fixtures.join(&file.file), directory.join(&file.file)).unwrap();
    }
    fs::write(
        accuracy_root(journal.path()).join(INSTALLED_RECORD),
        serde_json::to_vec(&installed).unwrap(),
    )
    .unwrap();
    crate::accuracy::release_loaded();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let scope = Scope::install(Arc::clone(&root)).unwrap();
    let packs = crate::accuracy::installed_packs(journal.path()).unwrap();
    assert!(packs.has_venues() && packs.has_containment());
    assert_eq!(packs.identity(), "venues:fixture-1;containment:fixture-1");
    // The maintained generation helper owns this path and shared exclusion.
    let legacy_use = crate::accuracy::asset_root(journal.path()).join("legacy-use/fixture-1");
    let other_reader = solstone_core_artifact_download::generation::hold_use(&legacy_use).unwrap();
    assert_eq!(
        solstone_core_artifact_download::generation::hold_write(&legacy_use)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    drop(scope);
    crate::accuracy::release_loaded();
    assert!(solstone_core_artifact_download::generation::hold_write(&legacy_use).is_err());
    drop(other_reader);
    drop(packs);
    solstone_core_artifact_download::generation::hold_write(&legacy_use).unwrap();
}

fn seed_location(journal: &Path) -> PathBuf {
    let config = journal.join("config/journal.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(config, r#"{"identity":{"timezone":"UTC"}}"#).unwrap();
    let source = journal.join("chronicle/20261001/phone/140000_300/location.jsonl");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, concat!(
        "{\"accuracy\":\"full\",\"gap\":false,\"kind\":\"location\",\"platform\":\"ios\",\"schema\":\"solstone.location.segment/1\",\"source\":\"location\",\"tier\":\"full\"}\n",
        "{\"h_acc\":10,\"lat\":39.7555,\"lon\":-105.2211,\"schema\":\"solstone.location.fix/1\",\"stationary\":false,\"t\":\"2026-10-01T14:00:10Z\"}\n"
    )).unwrap();
    source
}

#[test]
fn missing_document_with_current_record_keeps_index_retry_after_hint_refusal() {
    let journal = tempfile::tempdir().unwrap();
    let source = seed_location(journal.path());
    let input = fs::read(&source).unwrap();
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let mut reconciler = crate::Reconciler::default();
    let initial = crate::reconcile_journal_owned(Arc::clone(&root), &mut reconciler).unwrap();
    assert!(initial.failures.is_empty(), "{:?}", initial.failures);
    let document = crate::projection::document_path(journal.path(), "20261001");
    assert!(!fs::read(&document).unwrap().is_empty());
    assert!(crate::projection::read_build_record(journal.path(), "20261001").is_some());
    let hint = fs::read_dir(journal.path().join("indexer/todo"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    fs::remove_file(&document).unwrap();
    fs::write(&hint, b"{bad hint").unwrap();
    reconciler = crate::Reconciler::default();
    let failed = crate::reconcile_journal_owned(Arc::clone(&root), &mut reconciler).unwrap();
    assert!(!failed.failures.is_empty());
    assert!(document.is_file());
    assert!(
        crate::projection::read_build_record(journal.path(), "20261001").is_none(),
        "a restored current record cannot certify a missing index hint"
    );
    fs::remove_file(&hint).unwrap();
    let retried = crate::reconcile_journal_owned(root, &mut reconciler).unwrap();
    assert!(retried.failures.is_empty(), "{:?}", retried.failures);
    assert!(crate::projection::read_build_record(journal.path(), "20261001").is_some());
    let entry: solstone_core_indexer_store::todo::Entry =
        serde_json::from_slice(&fs::read(hint).unwrap()).unwrap();
    assert_eq!(entry.key, "20261001/location.md");
    assert_eq!(fs::read(source).unwrap(), input);
}

#[test]
fn bound_hint_keeps_existing_future_entry_and_refuses_corrupt_entry() {
    let journal = tempfile::tempdir().unwrap();
    let _scope = Scope::install(Arc::new(JournalRoot::open(journal.path()).unwrap())).unwrap();
    let path = Path::new("20261001/location.md");
    enqueue(journal.path(), path).unwrap();
    let hint = fs::read_dir(journal.path().join("indexer/todo"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut entry: solstone_core_indexer_store::todo::Entry =
        serde_json::from_slice(&fs::read(&hint).unwrap()).unwrap();
    entry.not_before_ms = u64::MAX;
    entry.attempts = 7;
    let future = serde_json::to_vec(&entry).unwrap();
    fs::write(&hint, &future).unwrap();
    enqueue(journal.path(), path).unwrap();
    assert_eq!(fs::read(&hint).unwrap(), future);
    fs::write(&hint, b"{bad hint").unwrap();
    assert!(enqueue(journal.path(), path).is_err());
    assert_eq!(fs::read(&hint).unwrap(), b"{bad hint");
}
