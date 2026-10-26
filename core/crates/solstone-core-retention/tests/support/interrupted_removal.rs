// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "disposable journals interrupted by real removals"
)]

use super::*;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::media::{
    MediaDirectory, MediaPrimitive, MediaStage, REMOVAL_ORIGINAL_LEAF, REMOVAL_RECORD_LEAF,
    run_with_media_hook,
};

const DAY: &str = "chronicle/20260701";
const SEGMENT: &str = "070000_17";

struct Journal {
    temp: tempfile::TempDir,
    root: Arc<JournalRoot>,
}

impl Journal {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(DAY).join(SEGMENT)).unwrap();
        fs::write(
            temp.path().join(DAY).join(SEGMENT).join("audio.flac"),
            b"original",
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        Self { temp, root }
    }
    fn raw(&self) -> PathBuf {
        self.temp.path().join(DAY).join(SEGMENT).join("audio.flac")
    }
    fn day(&self) -> PathBuf {
        self.temp.path().join(DAY)
    }
    /// Run a real removal and end it abruptly at `cut`, as a crash would: the
    /// owner and its lease are gone, and whatever it staged stays on disk.
    fn interrupt(&self, cut: MediaPrimitive) -> PathBuf {
        let activity = ActivityLease::try_acquire(Arc::clone(&self.root)).unwrap();
        let parent = MediaDirectory::open(Arc::clone(&self.root), Path::new(DAY)).unwrap();
        let source = parent.open_child(OsStr::new(SEGMENT)).unwrap().unwrap();
        let expected = source
            .observe_file(OsStr::new("audio.flac"))
            .unwrap()
            .unwrap();
        let stage = MediaStage::create(Arc::clone(&parent), Arc::clone(&activity)).unwrap();
        let path = stage.diagnostic_path();
        let ended = catch_unwind(AssertUnwindSafe(|| {
            run_with_media_hook(
                cut,
                || -> io::Result<()> { panic!("the process ended here") },
                || source.remove_observed(OsStr::new("audio.flac"), &expected, stage),
            )
        }));
        assert!(ended.is_err());
        assert!(path.is_dir());
        path
    }
    fn recover(&self) -> Vec<InterruptedRemoval> {
        let activity = ActivityLease::try_acquire(Arc::clone(&self.root)).unwrap();
        recover_interrupted_removals(&activity).expect("local recovery")
    }
}

#[test]
fn a_moved_original_returns_under_its_own_name() {
    let journal = Journal::new();
    let stage = journal.interrupt(MediaPrimitive::BeforeClaimDelete);
    assert!(!journal.raw().exists());
    assert_eq!(
        fs::read(stage.join(REMOVAL_ORIGINAL_LEAF)).unwrap(),
        b"original"
    );
    let recovered = journal.recover();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].outcome,
        Ok(InterruptedRemovalResolution::Returned)
    );
    assert_eq!(fs::read(journal.raw()).unwrap(), b"original");
    assert!(!stage.exists());
    assert!(journal.recover().is_empty());
}

#[test]
fn a_record_alone_is_discarded_whether_or_not_the_original_moved() {
    // Before the move the original is in place; after its deletion it is gone.
    for (cut, present) in [
        (MediaPrimitive::NativeClaimCut, true),
        (MediaPrimitive::AfterClaimDelete, false),
    ] {
        let journal = Journal::new();
        let stage = journal.interrupt(cut);
        assert!(stage.join(REMOVAL_RECORD_LEAF).is_file());
        assert!(!stage.join(REMOVAL_ORIGINAL_LEAF).exists());
        let recovered = journal.recover();
        assert_eq!(
            recovered[0].outcome,
            Ok(InterruptedRemovalResolution::Discarded)
        );
        assert_eq!(journal.raw().exists(), present);
        assert!(!stage.exists());
    }
}

#[test]
fn an_owner_deleted_segment_finishes_the_approved_deletion() {
    let journal = Journal::new();
    let stage = journal.interrupt(MediaPrimitive::AfterRemovalClaim);
    fs::write(journal.day().join(SEGMENT).join("tombstone.json"), b"{}").unwrap();
    let recovered = journal.recover();
    assert_eq!(
        recovered[0].outcome,
        Ok(InterruptedRemovalResolution::Finished)
    );
    assert!(!journal.raw().exists());
    assert!(!stage.exists());
}

#[test]
fn owner_deletion_admission_resolves_an_interrupted_removal() {
    let journal = Journal::new();
    let stage = journal.interrupt(MediaPrimitive::AfterRemovalClaim);
    let outcome = crate::door::recover(
        journal.temp.path(),
        "2026-07-02T00:00:00Z",
        crate::RemovalReason::OwnerSegmentDelete,
        "cid",
    );
    assert!(outcome.halted.is_none(), "{:?}", outcome.halted);
    assert_eq!(fs::read(journal.raw()).unwrap(), b"original");
    assert!(!stage.exists());
}

#[test]
fn lookalike_and_uncertain_stages_are_held_untouched() {
    let journal = Journal::new();
    // An offer-creation style copy under an archived media name.
    let copy = journal.day().join(".tmp-offload-copy");
    fs::create_dir(&copy).unwrap();
    fs::write(copy.join("audio.flac"), b"copy").unwrap();
    // A removal-shaped stage with something extra in it.
    let extra = journal.day().join(".tmp-offload-extra");
    fs::create_dir(&extra).unwrap();
    fs::write(extra.join(REMOVAL_RECORD_LEAF), b"{}").unwrap();
    fs::write(extra.join(REMOVAL_ORIGINAL_LEAF), b"original").unwrap();
    fs::write(extra.join("payload"), b"other").unwrap();
    let recovered = journal.recover();
    assert!(recovered.is_empty(), "{recovered:?}");
    assert_eq!(fs::read(copy.join("audio.flac")).unwrap(), b"copy");
    assert_eq!(fs::read(extra.join("payload")).unwrap(), b"other");

    // A moved original that no longer matches its record, or whose name was
    // taken meanwhile, is never put back or deleted.
    for drift in ["content", "occupied", "unreadable"] {
        let journal = Journal::new();
        let stage = journal.interrupt(MediaPrimitive::BeforeClaimDelete);
        match drift {
            "content" => fs::write(stage.join(REMOVAL_ORIGINAL_LEAF), b"changed!").unwrap(),
            "occupied" => fs::write(journal.raw(), b"newer").unwrap(),
            _ => fs::write(stage.join(REMOVAL_RECORD_LEAF), b"{").unwrap(),
        }
        let held = fs::read(stage.join(REMOVAL_ORIGINAL_LEAF)).unwrap();
        let recovered = journal.recover();
        assert_eq!(recovered.len(), 1, "{drift}");
        assert!(recovered[0].outcome.is_err(), "{drift}");
        assert_eq!(fs::read(stage.join(REMOVAL_ORIGINAL_LEAF)).unwrap(), held);
        assert!(stage.join(REMOVAL_RECORD_LEAF).is_file());
        if drift == "occupied" {
            assert_eq!(fs::read(journal.raw()).unwrap(), b"newer");
        } else {
            assert!(!journal.raw().exists());
        }
    }
}
