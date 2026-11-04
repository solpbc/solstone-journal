// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use super::*;
use crate::test_restore_intent::{fixture, register};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::atomic::{
    BoundPublicationPrimitive, run_with_bound_publication_fault,
};
use solstone_core_journal_io::media::{
    Cleanup, Confirmation, Durability, PublicationEffect, PublicationReceipt,
};
use solstone_core_retention::archive_catalog::codec::Event;
use solstone_core_retention::archive_proof::ArchiveFileProof;
use solstone_core_retention::restore_intent::{Obligation, PublicationDecision};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;

struct Fixture {
    temp: tempfile::TempDir,
    activity: Arc<ActivityLease>,
    intent: RestoreIntent,
}

impl Fixture {
    fn new(with_mark: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut intent = fixture(with_mark);
        fs::create_dir_all(temp.path().join(intent.locator.relative_segment())).unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let segment = iter_segments(
            root.canonical_path(),
            PathOrDay::Day(&intent.locator.target.day),
        )
        .unwrap()
        .remove(0);
        let destination =
            PreparedRestore::admit(Arc::clone(&root), &intent.locator.target.day, &segment)
                .unwrap();
        intent.destination = destination.destination().clone();
        intent.stage.parent = intent.destination.parent;
        let activity = ActivityLease::try_acquire(root).unwrap();
        let result = Self {
            temp,
            activity,
            intent,
        };
        result.append(&result.archive(
            &result.intent.locator.target.day,
            "manifest-1",
            "witness-a",
        ));
        fs::write(
            result.temp.path().join("health/retention-marks.json"),
            serde_json::to_vec(&register(&result.intent)).unwrap(),
        )
        .unwrap();
        result.persist();
        result
    }

    fn path(&self) -> PathBuf {
        self.temp
            .path()
            .join("health/offload/restore-intents")
            .join(intent_filename(&self.intent.event.operation_id).unwrap())
    }
    fn ledger(&self, day: &str) -> PathBuf {
        self.temp
            .path()
            .join("health/offload")
            .join(format!("{day}.jsonl"))
    }
    fn archive(&self, day: &str, manifest: &str, witness: &str) -> Event {
        let w = &self.intent.witness;
        Event::ArchiveV2 {
            schema_version: 2,
            time: 1,
            day: day.into(),
            stream: self.intent.locator.target.stream.clone(),
            segment: self.intent.locator.segment.clone(),
            segment_dir: self.intent.locator.target.dir.clone(),
            layout: self.intent.locator.layout,
            journal_id: w.journal_id.clone().unwrap(),
            manifest_id: manifest.into(),
            repository_id: w.repository_id.clone().unwrap(),
            snapshot_id: w.snapshot_id.clone(),
            files: vec![ArchiveFileProof {
                witness_id: witness.into(),
                name: w.file.name.clone(),
                bytes: w.file.bytes,
                sha256: w.file.sha256.clone(),
                archive_path: w.archive_path.clone().unwrap(),
                processing: w.processing.clone().unwrap(),
            }],
        }
    }
    fn append(&self, event: &Event) {
        let day = match event {
            Event::ArchiveV2 { day, .. } | Event::RestoreV2 { day, .. } => day,
            _ => panic!("fixture event"),
        };
        let path = self.ledger(day);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = fs::read(&path).unwrap_or_default();
        bytes.extend(serde_json::to_vec(event).unwrap());
        bytes.push(b'\n');
        fs::write(path, bytes).unwrap();
    }
    fn persist(&self) {
        let store = RestoreIntentStore::create(Arc::clone(&self.activity))
            .unwrap_or_else(|failure| panic!("{:?}", failure.problem));
        let write = if let Some(observed) = store.read(&self.intent.event.operation_id).unwrap() {
            store.update(&observed, self.intent.clone())
        } else {
            store.create_intent(self.intent.clone())
        };
        assert!(
            write.receipt.fully_confirmed(),
            "{:?}",
            write.receipt.failures
        );
    }
    fn selected(&self) -> NativeObligation {
        observe_obligations(self.activity.journal_root()).unwrap().into_entries().into_iter().find(|entry| {
            matches!(entry, NativeObligation::Intent { contents: IntentRead::Complete { operation_id, .. }, .. } if operation_id == &self.intent.event.operation_id)
        }).unwrap()
    }
    fn review(&self) -> RestartIntentReview {
        RestartIntentReview::open(Arc::clone(&self.activity), self.selected())
            .unwrap_or_else(|failure| panic!("{:?}", failure.problem))
    }
    fn inode_bytes(&self) -> (u64, Vec<u8>) {
        (
            fs::metadata(self.path()).unwrap().ino(),
            fs::read(self.path()).unwrap(),
        )
    }
    fn stage(&self, root: bool, digit: char) -> PathBuf {
        let parent = if root {
            self.temp.path().to_path_buf()
        } else {
            self.temp
                .path()
                .join(self.intent.locator.relative_segment())
                .parent()
                .unwrap()
                .to_path_buf()
        };
        let path = parent.join(format!(".tmp-offload-{}", digit.to_string().repeat(32)));
        fs::create_dir(&path).unwrap();
        path
    }
}

#[test]
fn unchanged_metadata_rewrite_keeps_every_stage_and_unrelated_intent_obligation() {
    let f = Fixture::new(true);
    let root_stage = f.stage(true, '1');
    let sibling_stage = f.stage(false, '2');
    fs::write(root_stage.join("unknown"), b"sole raw remains here").unwrap();
    let mut other = f.intent.clone();
    other.event.operation_id = "e".repeat(32);
    let other_path = f
        .path()
        .with_file_name(intent_filename(&other.event.operation_id).unwrap());
    fs::write(&other_path, other.encode().unwrap()).unwrap();
    let before = f.inode_bytes();
    let mark = fs::read(f.temp.path().join("health/retention-marks.json")).unwrap();
    let ledger = fs::read(f.ledger("20261009")).unwrap();
    let stage_ids = [
        fs::metadata(&root_stage).unwrap().ino(),
        fs::metadata(&sibling_stage).unwrap().ino(),
    ];
    let review = f.review();
    assert_eq!(review.owner.original.obligations.as_ref().unwrap().len(), 4);
    let nonce = *review.nonce();
    let result = review.confirm(&nonce);
    assert!(result.problem.is_none(), "{:?}", result.problem);
    let token = result.continuity.as_ref().unwrap();
    let write = result.review.owner.write.as_ref().unwrap();
    assert!(Arc::ptr_eq(&token.anchor, &write.owner.anchor));
    assert_eq!(token.intent, f.intent);
    assert_eq!(token.read, *write.receipt.confirmation.as_ref().unwrap());
    assert!(write.receipt.fully_confirmed());
    assert_eq!(write.receipt.durability, [Some(Durability::Synced); 4]);
    assert_ne!(before.0, f.inode_bytes().0);
    assert_eq!(before.1, f.inode_bytes().1);
    assert_eq!(
        stage_ids,
        [
            fs::metadata(root_stage).unwrap().ino(),
            fs::metadata(sibling_stage).unwrap().ino()
        ]
    );
    assert_eq!(fs::read(other_path).unwrap(), other.encode().unwrap());
    assert_eq!(
        fs::read(f.temp.path().join("health/retention-marks.json")).unwrap(),
        mark
    );
    assert_eq!(fs::read(f.ledger("20261009")).unwrap(), ledger);
    assert!(
        !f.temp
            .path()
            .join(f.intent.locator.relative_segment())
            .join("a.flac")
            .exists()
    );
    drop(result);
    assert_eq!(
        observe_obligations(f.activity.journal_root())
            .unwrap()
            .entries()
            .len(),
        4
    );
}

#[test]
fn wrong_nonce_and_identical_byte_replacement_refuse_without_rewrite() {
    for replaced in [false, true] {
        let f = Fixture::new(false);
        let review = f.review();
        let mut nonce = *review.nonce();
        if !replaced {
            nonce[0] ^= 1;
        }
        if replaced {
            let path = f.path().with_extension("replacement");
            fs::write(&path, fs::read(f.path()).unwrap()).unwrap();
            fs::rename(path, f.path()).unwrap();
        }
        let before = f.inode_bytes();
        let result = review.confirm(&nonce);
        assert!(result.continuity.is_none());
        assert!(result.review.owner.write.is_none());
        assert_eq!(f.inode_bytes(), before);
        assert!(Arc::ptr_eq(&result.review.owner.activity, &f.activity));
    }
}

#[test]
fn existing_only_admission_refuses_missing_parents_lock_and_torn_or_mixed_record() {
    for cut in 0..4 {
        let f = Fixture::new(false);
        let selected = f.selected();
        match cut {
            0 => fs::rename(
                f.path().parent().unwrap(),
                f.temp.path().join("saved-intents"),
            )
            .unwrap(),
            1 => {
                fs::remove_file(f.temp.path().join("health/offload/restore-intents.lock")).unwrap()
            }
            2 => fs::write(f.path(), b"{torn").unwrap(),
            _ => {
                let mut mixed = f.intent.clone();
                mixed.event.operation_id = "e".repeat(32);
                fs::write(f.path(), mixed.encode().unwrap()).unwrap();
            }
        }
        let result = RestartIntentReview::open(Arc::clone(&f.activity), selected)
            .err()
            .unwrap();
        assert!(Arc::ptr_eq(&result.owner.activity, &f.activity));
        assert!(result.owner.write.is_none());
        if cut == 0 {
            assert!(!f.path().parent().unwrap().exists());
        }
        if cut == 1 {
            assert!(
                !f.temp
                    .path()
                    .join("health/offload/restore-intents.lock")
                    .exists()
            );
        }
    }
}

#[test]
fn copied_destination_cannot_gain_record_continuity() {
    let f = Fixture::new(false);
    let copied = tempfile::tempdir().unwrap();
    fs::create_dir_all(copied.path().join(f.intent.locator.relative_segment())).unwrap();
    fs::create_dir_all(copied.path().join("health/offload/restore-intents")).unwrap();
    let path = copied
        .path()
        .join("health/offload/restore-intents")
        .join(f.path().file_name().unwrap());
    fs::copy(f.path(), &path).unwrap();
    fs::copy(
        f.ledger("20261009"),
        copied.path().join("health/offload/20261009.jsonl"),
    )
    .unwrap();
    fs::copy(
        f.temp.path().join("health/offload/restore-intents.lock"),
        copied.path().join("health/offload/restore-intents.lock"),
    )
    .unwrap();
    let activity =
        ActivityLease::try_acquire(Arc::new(JournalRoot::open(copied.path()).unwrap())).unwrap();
    let selected = observe_obligations(activity.journal_root())
        .unwrap()
        .into_entries()
        .remove(0);
    let before = (fs::metadata(&path).unwrap().ino(), fs::read(&path).unwrap());
    let failure = RestartIntentReview::open(Arc::clone(&activity), selected)
        .err()
        .unwrap();
    assert!(matches!(
        failure.problem,
        ReviewProblem::Repair(RepairHold::DestinationChanged)
    ));
    assert_eq!(
        (fs::metadata(&path).unwrap().ino(), fs::read(path).unwrap()),
        before
    );
    assert!(Arc::ptr_eq(&failure.owner.activity, &activity));
}

#[test]
fn fresh_stage_full_mark_history_event_destination_and_lock_changes_refuse_before_update() {
    for cut in 0..6 {
        let f = Fixture::new(true);
        let review = f.review();
        let nonce = *review.nonce();
        match cut {
            0 => {
                f.stage(true, '3');
            }
            1 => {
                let mut changed = register(&f.intent);
                changed
                    .marks
                    .values_mut()
                    .next()
                    .unwrap()
                    .marked_at
                    .push('x');
                fs::write(
                    f.temp.path().join("health/retention-marks.json"),
                    serde_json::to_vec(&changed).unwrap(),
                )
                .unwrap();
            }
            2 => f.append(&f.archive("20261009", "manifest-new", "witness-new")),
            3 => f.append(&Event::RestoreV2 {
                schema_version: 2,
                time: 2,
                day: "20261009".into(),
                stream: f.intent.locator.target.stream.clone(),
                segment: f.intent.locator.segment.clone(),
                operation_id: f.intent.event.operation_id.clone(),
                files: f.intent.event.files.clone(),
            }),
            4 => {
                let path = f.temp.path().join(f.intent.locator.relative_segment());
                fs::rename(&path, path.with_extension("moved")).unwrap();
            }
            _ => {
                let path = f.temp.path().join("health/offload/restore-intents.lock");
                fs::rename(&path, path.with_extension("old")).unwrap();
                fs::write(path, b"").unwrap();
            }
        }
        let before = f.inode_bytes();
        let result = review.confirm(&nonce);
        assert!(result.continuity.is_none(), "cut {cut}");
        assert!(result.review.owner.write.is_none(), "cut {cut}");
        assert_eq!(f.inode_bytes(), before);
    }
}

#[test]
fn cross_day_manifest_witness_and_operation_collisions_refuse_initial_review() {
    for cut in 0..3 {
        let f = Fixture::new(false);
        let (manifest, witness) = match cut {
            0 => ("manifest-1", "witness-other"),
            1 => ("manifest-other", "witness-a"),
            _ => ("manifest-other", "witness-other"),
        };
        f.append(&f.archive("20261008", manifest, witness));
        if cut == 2 {
            let mut files = f.intent.event.files.clone();
            files[0].witness_id = witness.into();
            f.append(&Event::RestoreV2 {
                schema_version: 2,
                time: 2,
                day: "20261008".into(),
                stream: f.intent.locator.target.stream.clone(),
                segment: f.intent.locator.segment.clone(),
                operation_id: f.intent.event.operation_id.clone(),
                files,
            });
        }
        assert!(
            !read_complete_catalog(f.activity.journal_root())
                .unwrap()
                .degraded()
        );
        let before = f.inode_bytes();
        let failure = RestartIntentReview::open(Arc::clone(&f.activity), f.selected())
            .err()
            .unwrap();
        assert!(matches!(
            failure.problem,
            ReviewProblem::Repair(RepairHold::CatalogChanged | RepairHold::OperationCollision)
        ));
        assert_eq!(f.inode_bytes(), before);
    }
}

#[test]
fn saved_complete_historical_event_never_masks_actual_raw_read_error() {
    let mut f = Fixture::new(false);
    // Synthetic saved history tests the decoder/guard, not Native publication.
    f.intent
        .record_outcome(&PublicationReceipt {
            effect: PublicationEffect::Conflict,
            confirmation: Confirmation::Unverified,
            file_durability: Durability::Unproven,
            parent_durability: Durability::Unproven,
            cleanup: Cleanup::Complete,
            identity: None,
            error: None,
        })
        .unwrap();
    f.persist(); // Persist the exact prospective event before recording it.
    f.intent.marker = Obligation::NotRequired;
    f.intent.event_recorded = Obligation::Complete;
    f.intent.validate().unwrap();
    f.persist();
    f.append(&Event::RestoreV2 {
        schema_version: 2,
        time: 2,
        day: "20261009".into(),
        stream: f.intent.locator.target.stream.clone(),
        segment: f.intent.locator.segment.clone(),
        operation_id: f.intent.event.operation_id.clone(),
        files: f.intent.event.files.clone(),
    });
    let raw = f
        .temp
        .path()
        .join(f.intent.locator.relative_segment())
        .join("a.flac");
    fs::write(f.temp.path().join("outside"), b"protected").unwrap();
    symlink(f.temp.path().join("outside"), &raw).unwrap();
    let segment = iter_segments(
        f.activity.journal_root().canonical_path(),
        PathOrDay::Day("20261009"),
    )
    .unwrap()
    .remove(0);
    let prepared =
        PreparedRestore::admit(Arc::clone(f.activity.journal_root()), "20261009", &segment)
            .unwrap();
    assert_eq!(
        prepared
            .inspect(
                &f.intent,
                LockOptions {
                    timeout: Duration::ZERO,
                    poll_interval: Duration::ZERO,
                    mode: Some(0o600)
                }
            )
            .unwrap()
            .publication,
        PublicationDecision::HistoricalOnly
    );
    let before = f.inode_bytes();
    let failure = RestartIntentReview::open(Arc::clone(&f.activity), f.selected())
        .err()
        .unwrap();
    assert!(matches!(failure.problem, ReviewProblem::Raw));
    assert!(failure.owner.original.raw.as_ref().unwrap().is_err());
    assert_eq!(f.inode_bytes(), before);
    assert_eq!(fs::read(raw).unwrap(), b"protected");
}

thread_local! {
    static AFTER_READS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn after_reads() {
    let callback = AFTER_READS.with(|slot| slot.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

struct LateHook {
    root: PathBuf,
    intent: PathBuf,
    cut: usize,
    fired: bool,
}
impl super::super::native::Hooks for LateHook {
    fn checkpoint(&mut self, point: super::super::native::Point) {
        if point != super::super::native::Point::AfterWrite {
            return;
        }
        if self.cut == 1 {
            let path = self.intent.with_extension("replacement");
            fs::write(&path, fs::read(&self.intent).unwrap()).unwrap();
            fs::rename(path, &self.intent).unwrap();
            self.fired = true;
        } else if self.cut == 2 {
            fs::create_dir(self.root.join(format!(".tmp-offload-{}", "9".repeat(32)))).unwrap();
            self.fired = true;
        } else if self.cut == 3 {
            let root = self.root.clone();
            AFTER_READS.with(|slot| {
                assert!(slot.borrow().is_none());
                *slot.borrow_mut() = Some(Box::new(move || {
                    fs::create_dir(root.join(format!(".tmp-offload-{}", "8".repeat(32)))).unwrap();
                }));
            });
            self.fired = true;
        }
    }
}

#[test]
fn late_sync_named_replacement_and_new_stage_keep_actual_publication_without_continuity() {
    for cut in 0..4 {
        let f = Fixture::new(false);
        let review = f.review();
        let nonce = *review.nonce();
        let before = f.inode_bytes();
        let mut hook = LateHook {
            root: f.temp.path().to_path_buf(),
            intent: f.path(),
            cut,
            fired: false,
        };
        let result = if cut == 0 {
            let (result, fired) = run_with_bound_publication_fault(
                BoundPublicationPrimitive::ParentSync,
                1,
                nix::libc::EIO,
                || review.confirm_with(&nonce, &mut hook),
            );
            assert!(fired);
            result
        } else {
            let result = review.confirm_with(&nonce, &mut hook);
            assert!(hook.fired);
            result
        };
        assert!(result.continuity.is_none());
        let write = result.review.owner.write.as_ref().unwrap();
        assert!(write.receipt.native.is_some());
        let published = match write.receipt.native.as_ref().unwrap() {
            super::super::native::NativePublication::Update(Ok(
                solstone_core_journal_io::atomic::BoundAtomicOutcome::Published { observation }
                | solstone_core_journal_io::atomic::BoundAtomicOutcome::PublishedDurabilityUncertain { observation, .. }
                | solstone_core_journal_io::atomic::BoundAtomicOutcome::PublishedObservationUncertain { observation, .. }
            )) => observation,
            _ => panic!("missing actual native publication"),
        };
        assert_ne!(published.entry.inode, before.0);
        assert_eq!(f.inode_bytes().1, before.1);
        assert!(Arc::ptr_eq(&write.owner.activity, &f.activity));
        if cut == 0 {
            assert!(!write.receipt.fully_confirmed());
            assert_eq!(write.receipt.durability[0], Some(Durability::Unproven));
        }
        if cut == 1 {
            assert!(!write.receipt.fully_confirmed());
        }
        if cut == 3 {
            assert!(write.receipt.fully_confirmed());
            assert!(
                !result
                    .review
                    .owner
                    .latest
                    .obligations
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, NativeObligation::Stage { .. }))
            );
            assert!(
                result
                    .review
                    .owner
                    .latest
                    .final_obligations
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, NativeObligation::Stage { .. }))
            );
            assert!(AFTER_READS.with(|slot| slot.borrow().is_none()));
        }
        if cut == 2 {
            assert!(write.receipt.fully_confirmed());
            assert!(
                result
                    .review
                    .owner
                    .latest
                    .obligations
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|entry| matches!(entry, NativeObligation::Stage { .. }))
            );
        }
    }
}

#[test]
fn existing_only_legacy_lock_policy_preserves_inode_and_created_store_makes_it_private() {
    let f = Fixture::new(false);
    let path = f.temp.path().join("health/offload/restore-intents.lock");
    let inode = fs::metadata(&path).unwrap().ino();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let review = f.review();
    let nonce = *review.nonce();
    let result = review.confirm(&nonce);
    assert!(result.continuity.is_some(), "{:?}", result.problem);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
    drop(result);
    // The strict store repairs a readable lock in place rather than refusing it.
    assert!(RestoreIntentStore::create(Arc::clone(&f.activity)).is_ok());
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
}
