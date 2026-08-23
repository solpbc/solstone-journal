// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use solstone_core_journal_io::JournalRoot;

use crate::digest::hex_encode;
use crate::layout::DayKey;
use crate::lock::DayLockSet;
use crate::store::ConvergenceStore;
use crate::{OrdinaryAuthority, OrdinaryIntent, PublishOutcome, check_initialized, initialize};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
        let path = PathBuf::from("/var/tmp").join(format!(
            "sjc-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create /var/tmp test directory");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn journal_path(&self) -> PathBuf {
        self.path.join("journal")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

thread_local! {
    static FAIL_DIR_SYNC: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn fail_next_dir_sync() {
    FAIL_DIR_SYNC.set(true);
}

pub(crate) fn take_fail_dir_sync() -> bool {
    FAIL_DIR_SYNC.replace(false)
}

pub(crate) fn open_root(temporary: &TempDir) -> (PathBuf, JournalRoot) {
    let journal = temporary.journal_path();
    fs::create_dir(&journal).expect("create journal root");
    let root = JournalRoot::open(&journal).expect("open journal root");
    (journal, root)
}

pub(crate) fn initialized_store() -> (TempDir, ConvergenceStore) {
    let temporary = TempDir::new("store");
    let (_, root) = open_root(&temporary);
    initialize(&root).expect("initialize");
    let store = ConvergenceStore::open(root).expect("open store");
    (temporary, store)
}

pub(crate) fn sample_day() -> DayKey {
    DayKey::parse("20260823").unwrap()
}

pub(crate) fn dirty(store: &ConvergenceStore, locks: &DayLockSet, day: &DayKey) -> PublishOutcome {
    let proof = store.allocate(locks).unwrap();
    let proposal = store
        .propose(locks, day, OrdinaryIntent::AdvanceDirty)
        .unwrap();
    let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
    store.publish(locks, day, &mut authority).unwrap()
}

pub(crate) fn days_dir(temporary: &TempDir) -> PathBuf {
    temporary.journal_path().join("health/convergence/days")
}

pub(crate) fn records_dir(temporary: &TempDir) -> PathBuf {
    temporary.journal_path().join("health/convergence/records")
}

pub(crate) fn snapshot_tree(root: &Path) -> BTreeMap<String, (u64, String)> {
    let mut entries = BTreeMap::new();
    snapshot_walk(root, root, &mut entries);
    entries
}

fn snapshot_walk(root: &Path, dir: &Path, entries: &mut BTreeMap<String, (u64, String)>) {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(_) => return,
    };
    for entry in listing.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("child of root")
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            entries.insert(rel, (0, "dir".to_owned()));
            snapshot_walk(root, &path, entries);
        } else if let Ok(bytes) = fs::read(&path) {
            entries.insert(
                rel,
                (bytes.len() as u64, hex_encode(&Sha256::digest(&bytes))),
            );
        }
    }
}

pub(crate) fn assert_not_initialized_creates_nothing() {
    let temporary = TempDir::new("check-empty");
    let (journal, root) = open_root(&temporary);
    let before = snapshot_tree(&journal);
    assert!(!check_initialized(&root).unwrap());
    let after = snapshot_tree(&journal);
    assert_eq!(before, after);
}
