// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use solstone_core_journal_io::JournalRoot;

use crate::digest::hex_encode;
use crate::error::{ConvergenceError, Refusal};
use crate::init::{check_initialized, initialize};
use crate::layout::DayKey;
use crate::owner::{AdmitOutcome, ClaimAdmission, OwnerBinding};
use crate::preflight::{Admitted, Preflight, preflight};
use crate::selector::{GrantRequestSelector, OperationId, TransactionClass};
use crate::store::ConvergenceStore;
use crate::transaction::HeldDays;

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
    static PUBLISH_FAULT: Cell<Option<PublishFault>> = const { Cell::new(None) };
    static AFTER_WITNESS: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static AFTER_DISCOVERY: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum PublishFault {
    AfterEver,
    AfterWitness,
    AfterHead,
    AfterRecord,
    AfterAdopt,
    AfterSerial,
    AfterClaimDir,
    AfterClaimRevision,
    AfterClaimHead,
    AfterIntent,
    AfterActive,
    AfterHealthDir,
    AfterProjectionStream,
    AfterDailyUnlink,
    AfterProjectionSync,
    AfterTerminalPrepub,
    AfterTerminal,
    AfterTerminalSync,
    AfterActiveClear,
    AfterIntentClear,
    AfterMemberA,
    AfterMemberB,
    AfterBarrier,
    AfterTerminalEvict,
    AfterReleaseRevision,
    AfterReleaseHead,
    AfterConsumeWitness,
    AfterConsumeUnlink,
    AfterConsumeSync,
    AfterPreparedOwner,
    AfterOwnerIntentLink,
    AfterOwnerIntentLinkSync,
}

/// Clears TLS injects if a test panics between arm and take.
pub(crate) struct InjectGuard;

impl Drop for InjectGuard {
    fn drop(&mut self) {
        FAIL_DIR_SYNC.set(false);
        PUBLISH_FAULT.set(None);
        AFTER_WITNESS.with(|slot| slot.borrow_mut().take());
        AFTER_DISCOVERY.with(|slot| slot.borrow_mut().take());
    }
}

pub(crate) fn fail_next_dir_sync() -> InjectGuard {
    FAIL_DIR_SYNC.set(true);
    InjectGuard
}

pub(crate) fn fail_after_witness() -> InjectGuard {
    PUBLISH_FAULT.set(Some(PublishFault::AfterWitness));
    InjectGuard
}

pub(crate) fn fail_after(fault: PublishFault) -> InjectGuard {
    PUBLISH_FAULT.set(Some(fault));
    InjectGuard
}

pub(crate) fn after_witness(hook: impl FnOnce() + 'static) -> InjectGuard {
    AFTER_WITNESS.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    InjectGuard
}

pub(crate) fn after_discovery(hook: impl FnOnce() + 'static) -> InjectGuard {
    AFTER_DISCOVERY.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    InjectGuard
}

pub(crate) fn take_fail_dir_sync() -> bool {
    FAIL_DIR_SYNC.replace(false)
}

pub(crate) fn take_publish_fault(expected: PublishFault) -> bool {
    match PUBLISH_FAULT.get() {
        Some(fault) if fault == expected => {
            PUBLISH_FAULT.set(None);
            true
        }
        _ => false,
    }
}

pub(crate) fn run_after_witness_hook() {
    if let Some(hook) = AFTER_WITNESS.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
}

pub(crate) fn run_after_discovery_hook() {
    if let Some(hook) = AFTER_DISCOVERY.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
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

pub(crate) fn admit_days(name: &str, days: &[&str]) -> (TempDir, Admitted) {
    let temporary = TempDir::new(name);
    let (_, root) = open_root(&temporary);
    let set = match preflight(days.iter().copied()).unwrap() {
        Preflight::Ready(set) => set,
        Preflight::Empty => panic!("days"),
    };
    let admitted = set.admit(root).unwrap();
    (temporary, admitted)
}

/// Prepared owner for a **fresh** external operation over the admitted days,
/// with an empty grant-request selector. A fresh operation per call keeps the
/// two-owner contention tests meaningful; same-operation retry tests construct
/// and reuse an `OperationId` explicitly.
pub(crate) fn prepared_owner(admitted: &Admitted) -> Result<OwnerBinding, ConvergenceError> {
    let operation = OperationId::generate()?;
    let selector = GrantRequestSelector::empty(admitted.days())?;
    OwnerBinding::prepare(
        admitted,
        &operation,
        TransactionClass::AdvanceDirty,
        &selector,
    )
}

/// Hook B, unwrapped to the proof. `ExistingLink` is a recovery outcome, not a
/// proof, so it is an error here.
pub(crate) fn admit_proof(
    held: &HeldDays<'_>,
    owner: &OwnerBinding,
) -> Result<ClaimAdmission, ConvergenceError> {
    match ClaimAdmission::admit(held, owner)? {
        AdmitOutcome::Proof(proof) => Ok(proof),
        AdmitOutcome::ExistingLink => Err(ConvergenceError::Refused(Refusal::ReusedAuthority)),
    }
}

pub(crate) fn continue_ok(admitted: &Admitted) -> HeldDays<'_> {
    let owner = prepared_owner(admitted).unwrap();
    let mut held = admitted.begin(owner).unwrap();
    let proof = admit_proof(&held, held.owner()).unwrap();
    held.continue_with(proof).unwrap();
    held
}

/// Later-dirty successor with its own fresh one-shot proof.
pub(crate) fn advance_dirty_ok(held: &mut HeldDays<'_>) {
    let proof = admit_proof(held, held.owner()).unwrap();
    held.advance_dirty(proof).unwrap();
}

/// Later-dirty successor, returning the refusal instead of panicking.
pub(crate) fn advance_dirty_err(held: &mut HeldDays<'_>) -> ConvergenceError {
    let proof = admit_proof(held, held.owner()).unwrap();
    held.advance_dirty(proof).unwrap_err()
}

pub(crate) fn continue_with_fault<'a>(
    admitted: &'a Admitted,
    fault: PublishFault,
) -> (HeldDays<'a>, crate::error::ConvergenceError) {
    let owner = prepared_owner(admitted).unwrap();
    let mut held = admitted.begin(owner).unwrap();
    let proof = admit_proof(&held, held.owner()).unwrap();
    let _guard = fail_after(fault);
    let error = held.continue_with(proof).unwrap_err();
    (held, error)
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
