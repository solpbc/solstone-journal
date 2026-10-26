// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use super::retirement::NativeRetirement;
use super::*;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::media::{
    MediaDigest, MediaDirectory, MediaPrimitive, run_with_media_hook,
};
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};
use solstone_core_retention::archive_proof::{ArchiveFileProof, OffloadOffer, OffloadProof};
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

mod cli_cleanup;
mod legacy;
mod settlement;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Point {
    AfterInitialMark,
    AfterInitialIntent,
    AfterPublication,
    BeforeNoEffectPersist,
    BeforeRestoredCas,
}

type Hook = (Point, Box<dyn FnOnce()>);
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }

pub(super) fn checkpoint(point: Point) {
    let hook = HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(expected, _)| *expected == point)
        {
            slot.take().map(|(_, callback)| callback)
        } else {
            None
        }
    });
    if let Some(hook) = hook {
        hook();
    }
}

fn with_hook<T>(point: Point, hook: impl FnOnce() + 'static, operation: impl FnOnce() -> T) -> T {
    HOOK.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some((point, Box::new(hook)));
    });
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            HOOK.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    let _clear = Clear;
    let result = operation();
    HOOK.with(|slot| assert!(slot.borrow().is_none(), "checkpoint did not fire"));
    result
}

fn marker_published(result: &TransactionOutcome<'_, '_>) -> bool {
    #[cfg(unix)]
    {
        result.marker.as_ref().is_some_and(Result::is_ok)
    }
    #[cfg(windows)]
    {
        result.marker_published
    }
}

fn options() -> LockOptions {
    LockOptions {
        timeout: Duration::ZERO,
        poll_interval: Duration::ZERO,
        mode: Some(0o600),
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    root: Arc<JournalRoot>,
    locator: solstone_core_retention::restore_intent::RestoreLocator,
    witness: ArchiveWitness,
    mark: Option<Mark>,
}

impl Fixture {
    fn new(with_mark: bool) -> Self {
        let template = crate::test_restore_intent::fixture(with_mark);
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(template.locator.relative_segment())).unwrap();
        fs::create_dir(temp.path().join("source")).unwrap();
        fs::write(temp.path().join("source/a.flac"), b"abc").unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let mut witness = template.witness.clone();
        witness.file.sha256 = format!("{:x}", Sha256::digest(b"abc"));
        witness.journal_id = Some(root.identity().observation_id());
        let file = ArchiveFileProof {
            witness_id: witness.witness_id.clone(),
            name: witness.file.name.clone(),
            bytes: witness.file.bytes,
            sha256: witness.file.sha256.clone(),
            archive_path: witness.archive_path.clone().unwrap(),
            processing: witness.processing.clone().unwrap(),
        };
        let mut proof = OffloadProof {
            journal_id: root.identity().observation_id(),
            manifest_id: witness.manifest_id.clone(),
            target: template.locator.target.clone(),
            layout: template.locator.layout,
            repository_id: witness.repository_id.clone().unwrap(),
            snapshot_id: witness.snapshot_id.clone(),
            files: vec![file.clone()],
        };
        let mark = template.retirement.expected().cloned().map(|mut mark| {
            let mut offered = mark.offload.as_ref().unwrap().proof.clone();
            offered.journal_id = proof.journal_id.clone();
            offered.files[0] = file;
            mark.offload = Some(OffloadOffer::new(offered.clone()).unwrap());
            proof = offered;
            mark
        });
        let written = crate::ledger::writer::append_archive_witness(&root, &proof, 1).unwrap();
        assert_eq!(written.confirmation(), LedgerConfirmation::Confirmed);
        if let Some(mark) = &mark {
            let mut register = Register::empty();
            register.version = 2;
            register.marks.insert(mark.id.clone(), mark.clone());
            fs::write(
                temp.path().join("health/retention-marks.json"),
                serde_json::to_vec(&register).unwrap(),
            )
            .unwrap();
        }
        Self {
            temp,
            root,
            locator: template.locator,
            witness,
            mark,
        }
    }

    fn raw(&self) -> PathBuf {
        self.temp
            .path()
            .join(self.locator.relative_segment())
            .join("a.flac")
    }
    fn ledger(&self) -> PathBuf {
        self.temp.path().join("health/offload/20261009.jsonl")
    }
    fn marker(&self) -> PathBuf {
        solstone_core_journal_io::health_marker::health_marker_path(
            self.temp.path(),
            &self.locator.target.day,
            solstone_core_journal_io::health_marker::HealthMarkerKind::Stream,
        )
    }

    fn staged<'a>(&self, allowed: &'a Cell<bool>) -> StagedRestore<'a, 'static> {
        let activity = ActivityLease::try_acquire(Arc::clone(&self.root)).unwrap();
        let carrier = Arc::new(
            RootActivityCarrier::prepare(Arc::clone(&activity))
                .unwrap_or_else(|error| panic!("carrier {:?}", error.error())),
        );
        self.staged_with_carrier(allowed, carrier)
    }

    fn staged_with_carrier<'a>(
        &self,
        allowed: &'a Cell<bool>,
        carrier: Arc<RootActivityCarrier>,
    ) -> StagedRestore<'a, 'static> {
        let activity = Arc::clone(carrier.activity());
        let segment = iter_segments(self.root.canonical_path(), PathOrDay::Day("20261009"))
            .unwrap()
            .remove(0);
        let destination = PreparedRestore::admit(Arc::clone(&self.root), "20261009", &segment)
            .unwrap_or_else(|error| panic!("admit {:?}", error.reason));
        let stage = MediaStage::create(destination.stage_parent(), activity).unwrap();
        let observation = stage.observation().unwrap();
        let source =
            MediaDirectory::open(Arc::clone(&self.root), std::path::Path::new("source")).unwrap();
        let expected = MediaDigest {
            size: self.witness.file.bytes,
            sha256: self.witness.file.sha256.clone(),
        };
        let media = stage
            .create_target()
            .unwrap()
            .copy_from(&source, OsStr::new(&self.witness.file.name), &expected)
            .unwrap();
        StagedRestore {
            stage: StageEvidence {
                parent: observation.parent_identity,
                directory: observation.directory_identity,
                leaf: media.identity(),
                stage_name: observation.directory_name,
                leaf_name: "payload".into(),
                boot_id: None,
            },
            witness: self.witness.clone(),
            legacy_binding: None,
            owner: Owner {
                carrier,
                destination,
                admission: Admission::LocalTest(allowed),
                media: Some(media),
            },
        }
    }
}

#[test]
fn actual_publish_records_intent_marker_and_exact_event_without_recount_on_replay() {
    use solstone_core_journal_io::health_marker::{
        DayMarkerPairStatus, HealthMarkerKind, HealthMarkerState, day_marker_pair_status,
        read_health_marker,
    };

    let f = Fixture::new(false);
    let day = &f.locator.target.day;
    assert_eq!(
        read_health_marker(f.temp.path(), day, HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Absent
    );
    assert_eq!(
        day_marker_pair_status(f.temp.path(), day).unwrap(),
        DayMarkerPairStatus::Complete
    );
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    assert!(result.hold.is_none(), "{:?}", result.hold);
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    let record = result.record.as_ref().unwrap();
    assert_eq!(record.intent.marker, Obligation::Complete);
    assert_eq!(record.intent.event_recorded, Obligation::Complete);
    assert_eq!(record.intent.cleanup, Obligation::Complete);
    assert!(record.writes.iter().all(StoreWriteReceipt::fully_confirmed));
    let HealthMarkerState::Versioned { marker, .. } =
        read_health_marker(f.temp.path(), day, HealthMarkerKind::Stream).unwrap()
    else {
        panic!("restore must publish the canonical stream marker");
    };
    assert_eq!(marker.generation, 1);
    #[cfg(unix)]
    assert_eq!(
        result
            .marker
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .generation(),
        marker.generation
    );
    assert_eq!(
        day_marker_pair_status(f.temp.path(), day).unwrap(),
        DayMarkerPairStatus::Dirty
    );
    assert!(
        !f.temp
            .path()
            .join("health/20261009/stream.updated")
            .exists()
    );
    #[cfg(unix)]
    {
        assert_eq!(result.marker_ancestor_sync.len(), 3);
        assert!(result.marker_ancestor_sync.iter().all(Result::is_ok));
    }
    let id = record.intent.event.operation_id.clone();
    let before = fs::read(f.ledger()).unwrap();
    let owner = result.owner;
    drop(result.record);
    let replayed = replay_local(owner, &id, 3, options());
    assert!(replayed.hold.is_none(), "{:?}", replayed.hold);
    assert_eq!(replayed.effects, AttemptEffects::default());
    assert_eq!(fs::read(f.ledger()).unwrap(), before);
    let HealthMarkerState::Versioned { marker: after, .. } =
        read_health_marker(f.temp.path(), day, HealthMarkerKind::Stream).unwrap()
    else {
        panic!("replay must preserve the canonical stream marker");
    };
    assert_eq!(after, marker);
}

#[test]
fn replay_claim_contention_retains_admitted_store_and_exact_record() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    assert!(result.hold.is_none());
    let intent = result.record.as_ref().unwrap().intent.clone();
    let owner = result.owner;
    drop(result.record);
    let _claims = OutputClaims::acquire([f.raw().with_extension("jsonl")], options()).unwrap();
    let replayed = replay_local(owner, &intent.event.operation_id, 3, options());
    assert!(matches!(
        replayed.hold,
        Some(Hold::Repair(RepairHold::ClaimsUnavailable))
    ));
    let record = replayed.record.as_ref().unwrap();
    assert!(record.store.is_some());
    assert_eq!(record.persisted.as_ref(), Some(&intent));
    assert_eq!(replayed.effects, AttemptEffects::default());
    assert!(replayed.ledger.is_none());
    assert!(replayed.register.is_none());
}

#[test]
fn identical_is_zero_bytes_while_different_and_nonfile_are_conflicts() {
    for kind in 0..3 {
        let f = Fixture::new(false);
        let allowed = Cell::new(true);
        if kind == 0 {
            fs::write(f.raw(), b"abc").unwrap();
        }
        if kind == 1 {
            fs::write(f.raw(), b"old").unwrap();
        }
        if kind == 2 {
            fs::create_dir(f.raw()).unwrap();
        }
        let result = f.staged(&allowed).commit(2, options());
        let record = result
            .record
            .as_ref()
            .unwrap_or_else(|| panic!("kind {kind}: {:?}", result.hold));
        assert_eq!(result.effects, AttemptEffects::default());
        assert_eq!(
            record.intent.event.files[0].disposition,
            if kind == 0 {
                RestoreDisposition::AlreadyPresent
            } else {
                RestoreDisposition::Conflict
            }
        );
        if kind == 1 {
            assert_eq!(fs::read(f.raw()).unwrap(), b"old");
        }
        if kind == 2 {
            assert!(f.raw().is_dir());
        }
        assert_eq!(record.intent.event_recorded, Obligation::Complete);
    }
}

#[test]
fn settled_conflict_preserves_original_mark_and_late_matching_bytes_without_reclassification() {
    for late_match in [false, true] {
        let f = Fixture::new(true);
        let allowed = Cell::new(true);
        fs::write(f.raw(), b"old").unwrap();
        let register = f.temp.path().join("health/retention-marks.json");
        let original = fs::read(&register).unwrap();
        let staged = f.staged(&allowed);
        let raw = f.raw();
        let result = with_hook(
            Point::AfterPublication,
            move || {
                if late_match {
                    fs::write(raw, b"abc").unwrap();
                }
            },
            || staged.commit(2, options()),
        )
        .retire_intent(options());
        assert!(result.hold.is_none(), "{:?}", result.hold);
        assert_eq!(result.effects, AttemptEffects::default());
        assert!(result.retirement.as_ref().unwrap().retired);
        assert!(result.no_effect_cleanup.as_ref().unwrap().confirmed());
        assert!(!marker_published(&result));
        assert!(result.register.is_none());
        assert!(!f.marker().exists());
        assert_eq!(fs::read(register).unwrap(), original);
        assert_eq!(
            fs::read(f.raw()).unwrap(),
            if late_match { b"abc" } else { b"old" }
        );
        let intent = &result.record.as_ref().unwrap().intent;
        assert_eq!(
            intent.event.files[0].disposition,
            RestoreDisposition::Conflict
        );
        assert_eq!(intent.marker, Obligation::NotRequired);
        assert_eq!(intent.register, Obligation::NotRequired);
        let catalog = read_complete_catalog(&f.root).unwrap();
        let operations = &catalog.days[0].segments[0].restores;
        assert_eq!(operations, std::slice::from_ref(&intent.event));
    }
}

#[test]
fn not_published_attempt_records_unattempted_and_retires_only_owned_metadata() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::BeforePublish,
        || Err(std::io::Error::other("before native publication")),
        || staged.commit(2, options()),
    );
    assert!(fired);
    let result = result.retire_intent(options());
    assert!(result.hold.is_none(), "{:?}", result.hold);
    assert_eq!(
        result.publication.as_ref().unwrap().effect,
        PublicationEffect::NotPublished
    );
    assert_eq!(result.effects, AttemptEffects::default());
    assert!(!f.raw().exists());
    assert!(!f.marker().exists());
    assert!(result.register.is_none());
    assert_eq!(
        result.record.as_ref().unwrap().intent.event.files[0].disposition,
        RestoreDisposition::Unattempted
    );
    assert!(result.no_effect_cleanup.as_ref().unwrap().confirmed());
    assert!(result.retirement.as_ref().unwrap().retired);
}

#[test]
fn no_effect_cleanup_and_parent_sync_failures_keep_obligations_and_store_exclusion() {
    for primitive in [MediaPrimitive::StageUnlink, MediaPrimitive::ParentSync] {
        let f = Fixture::new(false);
        let allowed = Cell::new(true);
        fs::write(f.raw(), b"old").unwrap();
        let staged = f.staged(&allowed);
        let stage = f
            .temp
            .path()
            .join("chronicle/20261009/camera")
            .join(&staged.stage.stage_name);
        let (result, fired) = run_with_media_hook(
            primitive,
            || Err(std::io::Error::other("no-effect cleanup cut")),
            || staged.commit(2, options()),
        );
        assert!(fired);
        let result = result.retire_intent(options()).into_owned();
        assert!(!result.completed());
        assert!(result.facts().store_exclusion_retained);
        assert!(result.facts().retirement.is_none());
        let cleanup = result.facts().no_effect_cleanup.unwrap();
        if primitive == MediaPrimitive::StageUnlink {
            assert_eq!(cleanup.cleanup, Cleanup::Pending);
            assert!(cleanup.parent_sync.is_none());
            assert!(stage.is_dir());
        } else {
            assert_eq!(cleanup.cleanup, Cleanup::Complete);
            assert!(cleanup.parent_sync.unwrap().is_err());
            assert!(!stage.exists());
        }
        assert_eq!(fs::read(f.raw()).unwrap(), b"old");
        assert!(!f.marker().exists());
    }
}

#[test]
fn no_effect_source_substitution_refuses_cleanup_and_preserves_both_files() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    fs::write(f.raw(), b"old").unwrap();
    let staged = f.staged(&allowed);
    let stage = f
        .temp
        .path()
        .join("chronicle/20261009/camera")
        .join(&staged.stage.stage_name);
    let payload = stage.join("payload");
    let saved = stage.join("admitted-payload");
    let changed = payload.clone();
    let retained = saved.clone();
    let result = with_hook(
        Point::AfterPublication,
        move || {
            #[cfg(unix)]
            {
                fs::rename(&changed, retained).unwrap();
                fs::write(changed, b"foreign").unwrap();
            }
            #[cfg(windows)]
            assert_eq!(
                fs::rename(&changed, retained).unwrap_err().raw_os_error(),
                Some(32)
            );
        },
        || staged.commit(2, options()),
    );
    #[cfg(windows)]
    {
        assert!(result.hold.is_none(), "{:?}", result.hold);
        assert_eq!(result.no_effect_cleanup.unwrap().cleanup, Cleanup::Complete);
        assert!(!saved.exists());
        assert!(!payload.exists());
        assert_eq!(fs::read(f.raw()).unwrap(), b"old");
    }
    #[cfg(unix)]
    {
        assert!(matches!(result.hold, Some(Hold::Retirement)));
        assert_eq!(
            result.no_effect_cleanup.as_ref().unwrap().cleanup,
            Cleanup::Refused
        );
        assert_eq!(fs::read(saved).unwrap(), b"abc");
        assert_eq!(fs::read(payload).unwrap(), b"foreign");
        assert_eq!(fs::read(f.raw()).unwrap(), b"old");
        assert!(result.retirement.is_none());
    }
}

#[test]
fn no_effect_ledger_uncertainty_stops_before_source_cleanup() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    fs::write(f.raw(), b"old").unwrap();
    let staged = f.staged(&allowed);
    let ledger = f.ledger();
    let result = with_hook(
        Point::AfterPublication,
        move || {
            let mut contents = fs::read(&ledger).unwrap();
            contents.extend_from_slice(b"{\n");
            fs::write(ledger, contents).unwrap();
        },
        || staged.commit(2, options()),
    );
    assert!(matches!(
        result.hold,
        Some(Hold::Repair(RepairHold::DegradedCatalog))
    ));
    assert!(result.no_effect_cleanup.is_none());
    assert!(result.retirement.is_none());
    assert_eq!(fs::read(f.raw()).unwrap(), b"old");
}

#[test]
#[cfg(unix)]
fn no_effect_final_write_uncertainty_retains_cleanup_receipt_and_intent() {
    use solstone_core_journal_io::atomic::{
        BoundPublicationPrimitive, run_with_bound_publication_fault,
    };
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    fs::write(f.raw(), b"old").unwrap();
    let staged = f.staged(&allowed);
    let (result, fired) = run_with_bound_publication_fault(
        BoundPublicationPrimitive::FileSync,
        4,
        nix::libc::EIO,
        || staged.commit(2, options()),
    );
    assert!(fired);
    assert!(result.hold.is_some());
    assert!(result.no_effect_cleanup.as_ref().unwrap().confirmed());
    assert!(
        !result
            .record
            .as_ref()
            .unwrap()
            .writes
            .last()
            .unwrap()
            .fully_confirmed()
    );
    let path = intent_path(&f, &result);
    let result = result.retire_intent(options()).into_owned();
    assert!(!result.completed());
    assert!(result.facts().store_exclusion_retained);
    assert!(result.facts().retirement.is_none());
    assert!(path.is_file());
    assert_eq!(fs::read(f.raw()).unwrap(), b"old");
}

#[test]
fn no_effect_changed_mark_stops_before_prepared_source_cleanup() {
    let f = Fixture::new(true);
    let allowed = Cell::new(true);
    fs::write(f.raw(), b"old").unwrap();
    let staged = f.staged(&allowed);
    let stage = f
        .temp
        .path()
        .join("chronicle/20261009/camera")
        .join(&staged.stage.stage_name);
    let path = f.temp.path().join("health/retention-marks.json");
    let mut register = review_context::load_at_root(&f.root).unwrap();
    register.marks.values_mut().next().unwrap().marked_at = "2026-10-10T00:00:01Z".into();
    let bytes = serde_json::to_vec(&register).unwrap();
    let expected = bytes.clone();
    let changed = path.clone();
    let result = with_hook(
        Point::AfterPublication,
        move || fs::write(changed, bytes).unwrap(),
        || staged.commit(2, options()),
    );
    assert!(matches!(
        result.hold,
        Some(Hold::Repair(RepairHold::MarkChanged))
    ));
    assert!(result.no_effect_cleanup.is_none());
    assert!(result.ledger.is_none());
    assert!(stage.is_dir());
    assert_eq!(fs::read(path).unwrap(), expected);
    assert_eq!(fs::read(f.raw()).unwrap(), b"old");
}

#[test]
#[cfg(unix)]
fn torn_initial_write_refuses_before_native_publication() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    use solstone_core_journal_io::atomic::{
        BoundPublicationPrimitive, run_with_bound_publication_fault,
    };
    let (result, fired) = run_with_bound_publication_fault(
        BoundPublicationPrimitive::FileSync,
        1,
        nix::libc::EIO,
        || staged.commit(2, options()),
    );
    assert!(fired);
    assert!(result.hold.is_some());
    assert!(result.publication.is_none());
    assert!(!f.raw().exists());
    assert_eq!(result.effects, AttemptEffects::default());
    assert!(
        result
            .record
            .as_ref()
            .is_some_and(|record| !record.writes[0].fully_confirmed())
    );
}

#[test]
fn actual_publication_survives_torn_outcome_persistence_without_fabricating_success() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let intent_dir = f.temp.path().join("health/offload/restore-intents");
    let result = with_hook(
        Point::AfterPublication,
        move || {
            let path = fs::read_dir(intent_dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|ext| ext == "json"))
                .unwrap();
            fs::write(path, b"{").unwrap();
        },
        || staged.commit(2, options()),
    );
    assert!(result.hold.is_some());
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    assert_eq!(
        result.publication.as_ref().unwrap().effect,
        PublicationEffect::Published
    );
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert_eq!(
        result
            .record
            .as_ref()
            .unwrap()
            .intent
            .outcome
            .as_ref()
            .unwrap()
            .effect,
        PublicationEffect::Published
    );
    assert!(!f.marker().exists());
}

#[test]
fn cancellation_after_publication_still_persists_actual_effect_then_halts_marker_event_cas() {
    let f = Fixture::new(false);
    let allowed = std::rc::Rc::new(Cell::new(true));
    let staged = f.staged(&allowed);
    let stop = std::rc::Rc::clone(&allowed);
    let result = with_hook(
        Point::AfterPublication,
        move || stop.set(false),
        || staged.commit(2, options()),
    );
    assert!(matches!(result.hold, Some(Hold::Admission(_))));
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    let record = result.record.as_ref().unwrap();
    assert_eq!(
        record
            .persisted
            .as_ref()
            .unwrap()
            .outcome
            .as_ref()
            .unwrap()
            .effect,
        PublicationEffect::Published
    );
    assert_eq!(record.intent.event_recorded, Obligation::Pending);
    assert!(!f.marker().exists());
    assert!(result.ledger.is_none());
    assert!(result.register.is_none());
}

#[test]
fn changed_full_mark_after_fixed_claim_discovery_is_preserved_before_intent_or_publish() {
    let f = Fixture::new(true);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let path = f.temp.path().join("health/retention-marks.json");
    let mut register = review_context::load_at_root(&f.root).unwrap();
    register.marks.values_mut().next().unwrap().marked_at = "2026-10-10T00:00:01Z".into();
    let changed = serde_json::to_vec(&register).unwrap();
    let expected = changed.clone();
    let changed_path = path.clone();
    let result = with_hook(
        Point::AfterInitialMark,
        move || fs::write(changed_path, changed).unwrap(),
        || staged.commit(2, options()),
    );
    assert!(matches!(
        result.hold,
        Some(Hold::Repair(RepairHold::MarkChanged))
    ));
    assert_eq!(fs::read(path).unwrap(), expected);
    assert!(!f.raw().exists());
    assert!(result.record.is_none());
    assert!(result.publication.is_none());
}

#[test]
fn subset_remainder_is_persisted_before_publication_and_typed_writer_hold_is_truthful() {
    let f = Fixture::new(true);
    let allowed = Cell::new(true);
    let original = fs::read(f.temp.path().join("health/retention-marks.json")).unwrap();
    let result = f.staged(&allowed).commit(2, options());
    assert!(matches!(result.hold, Some(Hold::Register)));
    let record = result.record.as_ref().unwrap();
    let solstone_core_retention::restore_intent::MarkRetirement::Exact { plan } =
        &record.intent.retirement
    else {
        panic!("no exact plan")
    };
    assert_eq!(plan.expected(), f.mark.as_ref().unwrap());
    assert_eq!(plan.replacement().unwrap().proposal.names, ["b.flac"]);
    assert_eq!(plan.replacement().unwrap().proposal.bytes, 7);
    assert_eq!(record.intent.event_recorded, Obligation::Complete);
    assert_eq!(record.intent.register, Obligation::Pending);
    assert!(matches!(
        result.register,
        Some(Err(RestoreRegisterRefusal::Bound(
            solstone_core_retention::mark_mutation::BoundMutationRefusal::Review(
                solstone_core_retention::review_context::ReviewRefusal::Register(
                    solstone_core_retention::marks::StoreError::WriterHeld
                )
            )
        )))
    ));
    assert_eq!(
        fs::read(f.temp.path().join("health/retention-marks.json")).unwrap(),
        original
    );
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod packaged;

#[test]
fn missing_recorded_publication_never_advances_replay_marker_or_cas() {
    let f = Fixture::new(true);
    let allowed = Cell::new(true);
    let mut result = f.staged(&allowed).commit(2, options());
    let record = result.record.as_mut().unwrap();
    let id = record.intent.event.operation_id.clone();
    assert_eq!(record.intent.event_recorded, Obligation::Complete);
    // Model loss of the former publication owner before restart/replay.
    drop(result.owner.media.take());
    fs::remove_file(f.raw()).unwrap();
    let before = fs::read(f.marker()).unwrap();
    let owner = result.owner;
    drop(result.record);
    let replayed = replay_local(owner, &id, 3, options());
    assert!(matches!(
        replayed.hold,
        Some(Hold::Repair(RepairHold::UncertainContent))
    ));
    assert_eq!(replayed.effects, AttemptEffects::default());
    assert!(replayed.register.is_none());
    assert_eq!(fs::read(f.marker()).unwrap(), before);
}

#[test]
fn unverified_publication_matching_bytes_never_becomes_successful_event_or_cas() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::AfterPublish,
        || Err(std::io::Error::other("postpublication cut")),
        || staged.commit(2, options()),
    );
    assert!(fired);
    assert!(matches!(
        result.hold,
        Some(Hold::Repair(RepairHold::UncertainContent))
    ));
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    let id = result
        .record
        .as_ref()
        .unwrap()
        .intent
        .event
        .operation_id
        .clone();
    assert_eq!(
        result.record.as_ref().unwrap().intent.event_recorded,
        Obligation::Pending
    );
    assert!(marker_published(&result)); // invalidate the real effect without claiming restore success
    let before = fs::read(f.ledger()).unwrap();
    let owner = result.owner;
    drop(result.record);
    let replayed = replay_local(owner, &id, 3, options());
    assert!(matches!(
        replayed.hold,
        Some(Hold::Repair(RepairHold::UncertainContent))
    ));
    assert_eq!(replayed.effects, AttemptEffects::default());
    assert!(replayed.ledger.is_none());
    assert!(replayed.register.is_none());
    assert_eq!(fs::read(f.ledger()).unwrap(), before);
}

#[test]
#[cfg(unix)]
fn published_parent_sync_failure_holds_event_and_replay_even_when_bytes_match() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::ParentSync,
        || Err(std::io::Error::other("publication parent sync cut")),
        || staged.commit(2, options()),
    );
    assert!(fired);
    assert!(matches!(result.hold, Some(Hold::Retirement)));
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    let publication = result.publication.as_ref().unwrap();
    assert_eq!(publication.confirmation, Confirmation::Confirmed);
    assert_eq!(publication.parent_durability, Durability::Unproven);
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    let record = result.record.as_ref().unwrap();
    let id = record.intent.event.operation_id.clone();
    assert_eq!(record.intent.marker, Obligation::Complete);
    assert!(marker_published(&result));
    assert_eq!(record.intent.event_recorded, Obligation::Pending);
    assert!(result.ledger.is_none());
    assert!(result.register.is_none());
    let before = fs::read(f.ledger()).unwrap();
    let owner = result.owner;
    drop(result.record);
    let (replayed, _) = run_with_media_hook(
        MediaPrimitive::ParentSync,
        || Err(std::io::Error::other("replay parent sync cut")),
        || replay_local(owner, &id, 3, options()),
    );
    assert!(matches!(replayed.hold, Some(Hold::Retirement)));
    assert_eq!(replayed.effects, AttemptEffects::default());
    assert!(replayed.ledger.is_none());
    assert!(replayed.register.is_none());
    assert_eq!(fs::read(f.ledger()).unwrap(), before);
}

#[test]
#[cfg(windows)]
fn unsupported_directory_sync_permits_publication_and_replay() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::ParentSync,
        || Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        || staged.commit(2, options()),
    );
    assert!(fired);
    let publication = result.publication.as_ref().unwrap();
    assert_eq!(publication.confirmation, Confirmation::Confirmed);
    assert_eq!(publication.file_durability, Durability::Synced);
    assert_eq!(publication.parent_durability, Durability::Unproven);
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert!(!matches!(result.hold, Some(Hold::Retirement)));
    let record = result.record.as_ref().unwrap();
    let id = record.intent.event.operation_id.clone();
    assert_eq!(record.intent.marker, Obligation::Complete);
    assert!(marker_published(&result));
    if result.hold.is_none() {
        assert_eq!(record.intent.event_recorded, Obligation::Complete);
        assert!(result.ledger.is_some());
    }
    let before = fs::read(f.ledger()).unwrap();
    let owner = result.owner;
    drop(result.record);
    let (replayed, _) = run_with_media_hook(
        MediaPrimitive::ParentSync,
        || Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        || replay_local(owner, &id, 3, options()),
    );
    assert!(!matches!(replayed.hold, Some(Hold::Retirement)));
    assert_eq!(replayed.effects, AttemptEffects::default());
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert_eq!(fs::read(f.ledger()).unwrap(), before);
}

#[test]
fn marker_failure_preserves_published_effect_and_pending_event() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    fs::create_dir_all(f.marker().parent().unwrap()).unwrap();
    fs::write(f.marker(), b"broken marker").unwrap();
    let result = staged.commit(2, options());
    assert!(matches!(result.hold, Some(Hold::Marker)));
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    let record = result.record.as_ref().unwrap();
    assert_eq!(
        record
            .persisted
            .as_ref()
            .unwrap()
            .outcome
            .as_ref()
            .unwrap()
            .effect,
        PublicationEffect::Published
    );
    assert_eq!(record.intent.marker, Obligation::Pending);
    assert_eq!(record.intent.event_recorded, Obligation::Pending);
    assert!(result.ledger.is_none());
    assert_eq!(fs::read(f.marker()).unwrap(), b"broken marker");
}

#[test]
fn late_degraded_catalog_still_invalidates_actual_publication_then_holds_event() {
    use std::io::Write;
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let path = f.ledger();
    let result = with_hook(
        Point::AfterPublication,
        move || {
            fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"{\n")
                .unwrap();
        },
        || staged.commit(2, options()),
    );
    assert!(matches!(
        result.hold,
        Some(Hold::Repair(RepairHold::DegradedCatalog))
    ));
    assert_eq!(result.effects, AttemptEffects { files: 1, bytes: 3 });
    assert_eq!(
        result.record.as_ref().unwrap().intent.marker,
        Obligation::Complete
    );
    assert_eq!(
        result.record.as_ref().unwrap().intent.event_recorded,
        Obligation::Pending
    );
    assert!(result.ledger.is_none());
    assert!(fs::read(f.ledger()).unwrap().ends_with(b"{\n"));
}

#[test]
fn late_owner_delete_or_missing_segment_refuses_without_recreation_or_intent() {
    for missing in [false, true] {
        let f = Fixture::new(false);
        let allowed = Cell::new(true);
        let staged = f.staged(&allowed);
        let segment = f.raw().parent().unwrap().to_path_buf();
        if missing {
            #[cfg(unix)]
            fs::rename(&segment, segment.with_extension("moved")).unwrap();
            #[cfg(windows)]
            {
                assert_eq!(
                    fs::rename(&segment, segment.with_extension("moved"))
                        .unwrap_err()
                        .raw_os_error(),
                    Some(32)
                );
                assert!(segment.is_dir());
                continue;
            }
        } else {
            fs::write(segment.join("tombstone.json"), b"owner deleted").unwrap();
        }
        let result = staged.commit(2, options());
        assert!(matches!(
            result.hold,
            Some(Hold::Repair(RepairHold::DestinationChanged))
        ));
        assert!(result.record.is_none());
        assert!(result.publication.is_none());
        assert_eq!(result.effects, AttemptEffects::default());
        if missing {
            assert!(!segment.exists());
        } else {
            assert_eq!(
                fs::read(segment.join("tombstone.json")).unwrap(),
                b"owner deleted"
            );
        }
    }
}

fn intent_path(f: &Fixture, result: &TransactionOutcome<'_, '_>) -> PathBuf {
    f.temp
        .path()
        .join("health/offload/restore-intents")
        .join(format!(
            "{}.json",
            result.record.as_ref().unwrap().intent.event.operation_id
        ))
}

#[test]
fn retirement_deletes_only_the_confirmed_metadata_record_and_keeps_root_carrier() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    let path = intent_path(&f, &result);
    let ledger = fs::read(f.ledger()).unwrap();
    let marker = fs::read(f.marker()).unwrap();
    let retired = result.retire_intent(options());
    assert!(retired.hold.is_none(), "{:?}", retired.hold);
    assert!(retired.retirement.as_ref().unwrap().retired);
    assert!(!path.exists());
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
    assert_eq!(fs::read(f.marker()).unwrap(), marker);
    let census = observe_with_carrier(&f.root, retired.owner.carrier.observation()).unwrap();
    assert_eq!(census.entries().len(), 1);
    assert!(census.unresolved_entries().next().is_none());
}

#[test]
fn retirement_refuses_identical_replacement_before_first_read_and_complete_replay_without_token() {
    for replace in [true, false] {
        let f = Fixture::new(false);
        let allowed = Cell::new(true);
        let result = f.staged(&allowed).commit(2, options());
        let path = intent_path(&f, &result);
        let bytes = fs::read(&path).unwrap();
        let result = if replace {
            fs::rename(&path, f.temp.path().join("saved-original-intent")).unwrap();
            fs::write(&path, &bytes).unwrap();
            result
        } else {
            let id = result
                .record
                .as_ref()
                .unwrap()
                .intent
                .event
                .operation_id
                .clone();
            let owner = result.owner;
            drop(result.record);
            let replayed = replay_local(owner, &id, 3, options());
            assert!(replayed.hold.is_none());
            assert!(replayed.record.as_ref().unwrap().writes.is_empty());
            replayed
        };
        let retired = result.retire_intent(options());
        assert!(matches!(
            retired.hold,
            Some(Hold::Retirement | Hold::Store(StoreProblem::Changed))
        ));
        assert!(retired.retirement.is_none());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn retirement_keeps_pending_register_and_unknown_stage() {
    for with_mark in [true, false] {
        let f = Fixture::new(with_mark);
        let allowed = Cell::new(true);
        let result = f.staged(&allowed).commit(2, options());
        let path = intent_path(&f, &result);
        let bytes = fs::read(&path).unwrap();
        let unknown = f.temp.path().join("health/offload/.tmp-offload-unknown");
        if !with_mark {
            fs::create_dir(&unknown).unwrap();
        }
        let retired = result.retire_intent(options());
        assert!(matches!(
            retired.hold,
            Some(Hold::Register | Hold::Obligations)
        ));
        assert!(retired.retirement.is_none());
        assert_eq!(fs::read(path).unwrap(), bytes);
        if !with_mark {
            assert!(unknown.is_dir());
        }
    }
}

#[test]
fn retirement_keeps_actual_unused_stage_after_successful_creation_then_preclaim_refusal() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    let path = intent_path(&f, &result);
    let bytes = fs::read(&path).unwrap();
    let moved = f.temp.path().join("saved-original-intent");
    let replacement_path = path.clone();
    let replacement_bytes = bytes.clone();
    let (retired, fired) = run_with_media_hook(
        MediaPrimitive::StageCreated,
        move || {
            fs::rename(&replacement_path, moved)?;
            fs::write(&replacement_path, replacement_bytes)?;
            Ok(())
        },
        || result.retire_intent(options()),
    );
    assert!(fired);
    assert!(matches!(
        retired.hold,
        Some(Hold::Store(StoreProblem::Changed))
    ));
    let facts = retired.retirement.as_ref().unwrap();
    let NativeRetirement::Unconsumed(stage) = &facts.native else {
        panic!("stage ownership lost")
    };
    assert!(stage.diagnostic_path().is_dir());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(!facts.retired);
}

#[test]
fn retirement_interrupted_claim_preserves_exact_record_in_census_visible_stage() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    let path = intent_path(&f, &result);
    let bytes = fs::read(&path).unwrap();
    let (retired, fired) = run_with_media_hook(
        MediaPrimitive::AfterRemovalClaim,
        || Err(std::io::Error::other("claimed intent cut")),
        || result.retire_intent(options()),
    );
    assert!(fired);
    assert!(matches!(retired.hold, Some(Hold::Retirement)));
    let facts = retired.retirement.as_ref().unwrap();
    let NativeRetirement::Removal(removal) = &facts.native else {
        panic!("claim ownership lost")
    };
    assert_eq!(
        removal.receipt().effect,
        solstone_core_journal_io::media::RemovalEffect::Claimed
    );
    assert!(!path.exists());
    let stage = removal.stage_observation().unwrap();
    assert_eq!(
        fs::read(
            f.temp
                .path()
                .join("health/offload")
                .join(&stage.directory_name)
                .join(solstone_core_journal_io::media::REMOVAL_ORIGINAL_LEAF)
        )
        .unwrap(),
        bytes
    );
    let census = observe_with_carrier(&f.root, retired.owner.carrier.observation()).unwrap();
    assert!(census.unresolved_entries().any(|entry| matches!(
        entry,
        solstone_core_journal_io::activity_obligations::NativeObligation::Stage { .. }
    )));
    assert!(!facts.retired);
}

#[test]
fn retirement_native_sync_failure_keeps_deleted_effect_and_refuses_completion() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    let path = intent_path(&f, &result);
    let (retired, fired) = run_with_media_hook(
        MediaPrimitive::RemovalSourceSync,
        || Err(std::io::Error::other("intent source sync cut")),
        || result.retire_intent(options()),
    );
    assert!(fired);
    assert!(matches!(retired.hold, Some(Hold::Retirement)));
    let facts = retired.retirement.as_ref().unwrap();
    let NativeRetirement::Removal(removal) = &facts.native else {
        panic!("removal receipt lost")
    };
    assert_eq!(
        removal.receipt().effect,
        solstone_core_journal_io::media::RemovalEffect::Deleted
    );
    assert_eq!(removal.receipt().source_durability, Durability::Unproven);
    assert!(!path.exists());
    assert!(!facts.retired);
}

#[test]
fn retirement_preserves_actual_deleted_effect_when_final_census_changes() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let result = f.staged(&allowed).commit(2, options());
    let path = intent_path(&f, &result);
    let unknown = f.temp.path().join("health/offload/.tmp-offload-final");
    let created = unknown.clone();
    let (retired, fired) = run_with_media_hook(
        MediaPrimitive::RemovalParentSync,
        move || fs::create_dir(created),
        || result.retire_intent(options()),
    );
    assert!(fired);
    assert!(matches!(
        retired.hold,
        Some(Hold::Retirement | Hold::Obligations)
    ));
    assert!(!path.exists());
    let facts = retired.retirement.as_ref().unwrap();
    let NativeRetirement::Removal(removal) = &facts.native else {
        panic!("removal receipt lost")
    };
    assert_eq!(
        removal.receipt().effect,
        solstone_core_journal_io::media::RemovalEffect::Deleted
    );
    assert!(!facts.retired);
    assert!(unknown.is_dir());
}

#[test]
fn capacity_exact_boundary_passes_and_one_below_or_query_failure_refuses() {
    assert!(capacity_allows(Ok(100), 100).is_ok());
    assert!(capacity_allows(Ok(99), 100).is_err());
    assert!(capacity_allows(Ok(100), 0).is_err());
    assert!(capacity_allows(Err(std::io::Error::other("capacity query failed")), 100).is_err());
}

#[test]
fn detached_confirmed_retirement_releases_store_for_second_file_under_original_carrier() {
    two_files_under_original_carrier(false);
}

#[test]
fn settled_conflict_releases_store_for_next_file_without_earning_restored_bytes() {
    two_files_under_original_carrier(true);
}

fn two_files_under_original_carrier(conflict: bool) {
    let mut f = Fixture::new(false);
    let allowed = Cell::new(true);
    if conflict {
        fs::write(f.raw(), b"old").unwrap();
    }
    let first = f.staged(&allowed);
    let carrier = Arc::clone(&first.owner.carrier);
    let mut second = f.witness.clone();
    second.file.name = "b.flac".into();
    second.manifest_id = "second-manifest".into();
    second.witness_id = "second-witness".into();
    second.archive_path = Some("/source/b.flac".into());
    fs::write(f.temp.path().join("source/b.flac"), b"abc").unwrap();
    let proof = OffloadProof {
        journal_id: second.journal_id.clone().unwrap(),
        manifest_id: second.manifest_id.clone(),
        target: f.locator.target.clone(),
        layout: f.locator.layout,
        repository_id: second.repository_id.clone().unwrap(),
        snapshot_id: second.snapshot_id.clone(),
        files: vec![ArchiveFileProof {
            witness_id: second.witness_id.clone(),
            name: second.file.name.clone(),
            bytes: second.file.bytes,
            sha256: second.file.sha256.clone(),
            archive_path: second.archive_path.clone().unwrap(),
            processing: second.processing.clone().unwrap(),
        }],
    };
    crate::ledger::writer::append_archive_witness(&f.root, &proof, 3).unwrap();
    let first = first
        .commit(2, options())
        .retire_intent(options())
        .into_owned();
    assert!(first.completed());
    let facts = first.facts();
    assert!(!facts.store_exclusion_retained);
    assert!(facts.retirement.unwrap().retired);
    assert_eq!(
        facts.publication.unwrap().effect,
        if conflict {
            PublicationEffect::Conflict
        } else {
            PublicationEffect::Published
        }
    );
    let first_effects = if conflict {
        AttemptEffects::default()
    } else {
        AttemptEffects { files: 1, bytes: 3 }
    };
    assert_eq!(first.effects(), first_effects);
    assert_eq!(
        fs::read(f.raw()).unwrap(),
        if conflict { b"old" } else { b"abc" }
    );

    f.witness = second;
    let second = f
        .staged_with_carrier(&allowed, Arc::clone(&carrier))
        .commit(4, options())
        .retire_intent(options())
        .into_owned();
    assert!(second.completed(), "{:?}", second.facts().hold);
    assert!(!second.facts().store_exclusion_retained);
    assert_eq!(second.effects(), AttemptEffects { files: 1, bytes: 3 });
    assert_eq!(fs::read(f.raw().with_file_name("b.flac")).unwrap(), b"abc");
    let (expected_files, expected_bytes) = if conflict { (1, 3) } else { (2, 6) };
    assert_eq!(
        first.effects().files + second.effects().files,
        expected_files
    );
    assert_eq!(
        first.effects().bytes + second.effects().bytes,
        expected_bytes
    );
    assert!(carrier.revalidate().is_ok());
    assert!(first.facts().preparation.as_ref().unwrap().confirmed);
    crate::restore::owned::tests::check_actual_accounting(
        vec![first, second],
        expected_files,
        expected_bytes,
        false,
        conflict,
    );
}

#[test]
fn detached_hold_keeps_store_exclusion_and_published_bytes() {
    let f = Fixture::new(true);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let activity = Arc::clone(staged.owner.carrier.activity());
    let outcome = staged
        .commit(2, options())
        .retire_intent(options())
        .into_owned();
    assert!(!outcome.completed());
    assert_eq!(outcome.effects(), AttemptEffects { files: 1, bytes: 3 });
    assert!(outcome.facts().store_exclusion_retained);
    assert_eq!(
        outcome.facts().publication.unwrap().effect,
        PublicationEffect::Published
    );
    let refused = RestoreIntentStore::create(activity).err().unwrap();
    assert_eq!(refused.problem, StoreProblem::Busy);
    crate::restore::owned::tests::check_actual_accounting(vec![outcome], 1, 3, true, false);
}

#[test]
fn actual_stage_capacity_query_failure_preserves_empty_stage_before_target_issuance() {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let staged = f.staged(&allowed);
    let parent = staged.owner.destination.stage_parent();
    let stage = MediaStage::create(
        Arc::clone(&parent),
        Arc::clone(staged.owner.carrier.activity()),
    )
    .unwrap();
    let observed = stage.observation().unwrap();
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::CapacityQuery,
        || Err(std::io::Error::other("capacity unavailable")),
        || capacity_allows(stage.available_bytes(), 1),
    );
    assert!(fired);
    assert!(result.is_err());
    assert_eq!(stage.observation().unwrap(), observed);
    drop(stage);
    let directory = parent
        .open_child(OsStr::new(&observed.directory_name))
        .unwrap()
        .unwrap();
    assert_eq!(directory.identity(), observed.directory_identity);
    assert!(directory.entries(8).unwrap().unwrap().is_empty());
}
