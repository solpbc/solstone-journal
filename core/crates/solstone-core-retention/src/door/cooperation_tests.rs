// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "native fixture assertions"
)]

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use solstone_core_journal_io::media::{MediaDirectory, MediaStage};

use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Phase {
    AfterRaw,
    BeforeRawCas,
    AfterPivot,
    BeforeRestore,
    BeforeMarker,
}

type Checkpoint = Box<dyn FnMut(Phase, &Target)>;
thread_local! {
    static CHECKPOINT: RefCell<Option<Checkpoint>> = const { RefCell::new(None) };
}

pub(crate) fn checkpoint(phase: Phase, target: &Target) {
    CHECKPOINT.with(|slot| {
        if let Some(callback) = slot.borrow_mut().as_mut() {
            callback(phase, target);
        }
    });
}

fn with_checkpoint<T>(
    callback: impl FnMut(Phase, &Target) + 'static,
    action: impl FnOnce() -> T,
) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            CHECKPOINT.with(|slot| *slot.borrow_mut() = None);
        }
    }
    CHECKPOINT.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(Box::new(callback));
    });
    let _clear = Clear;
    action()
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "whole-segment-cooperation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self { root }
    }

    fn target(stream: &str, dir: &str) -> Target {
        Target {
            day: "20261011".into(),
            stream: stream.into(),
            dir: dir.into(),
        }
    }

    fn populate(&self, target: &Target) -> PathBuf {
        let path = self.root.join(segment_rel(target));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("audio.flac"), b"sole raw bytes").unwrap();
        fs::write(path.join("audio.jsonl"), b"derived bytes").unwrap();
        path
    }

    fn activity(&self) -> Arc<ActivityLease> {
        ActivityLease::try_acquire(Arc::new(JournalRoot::open(&self.root).unwrap())).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn remove_at(fixture: &Fixture, target: &Target) -> Outcome {
    remove_segments(
        &fixture.root,
        std::slice::from_ref(target),
        "2026-10-11T00:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    )
}

fn borrowed_remove(
    activity: &Arc<ActivityLease>,
    carrier: Option<&RootActivityCarrier>,
    targets: &[Target],
) -> Outcome {
    remove_segments_with_activity(
        activity,
        carrier,
        targets,
        "2026-10-11T00:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    )
}

fn seed_proven(fixture: &Fixture, target: &Target, name: &str) -> ProvenRaw {
    use crate::content::{ClosedHandlerSet, JournalMedia};
    use crate::eligibility::{FileVerdict, resolve_file};
    let directory = fixture.root.join(segment_rel(target));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join(name), b"raw").unwrap();
    let stem = name.rsplit_once('.').unwrap().0;
    let header = serde_json::json!({"segment": target.dir, "_solstone_processing": {
        "schema":"solstone.processing.v1", "state":"empty", "reason_code":"no_decodable_audio",
        "handler":"transcribe", "attempted_at":"2026-10-11T00:00:00Z", "input_size":3
    }});
    fs::write(
        directory.join(format!("{stem}.jsonl")),
        format!("{header}\n"),
    )
    .unwrap();
    let found = crate::scan::scan_segment(&directory, &ClosedHandlerSet, &JournalMedia);
    let found = found
        .iter()
        .find(|item| item.name.as_str() == name)
        .unwrap();
    match resolve_file(
        &ClosedHandlerSet,
        &target.day,
        &target.stream,
        &target.dir,
        &found.name,
        found.size,
        &found.sidecar,
    ) {
        FileVerdict::Releasable(proven) => proven,
        FileVerdict::Held(reason) => panic!("actual processing fixture held: {reason:?}"),
    }
}

#[test]
fn raw_path_busy_and_same_activity_positive_preserve_sidecars_and_evidence() {
    let fixture = Fixture::new();
    let target = Fixture::target("audio", "070000_17");
    let proven = seed_proven(&fixture, &target, "audio.flac");
    let activity = fixture.activity();
    let (held, tally) = release_raw(
        &fixture.root,
        std::slice::from_ref(&proven),
        RawReleaseClass::Policy,
        DateTime::UNIX_EPOCH,
    );
    assert!(held.targets[0].removed.is_empty());
    assert_eq!(tally.on_record, 0);
    assert!(fixture.root.join(proven.rel()).exists());
    assert!(
        !fixture
            .root
            .join(proven.segment_rel())
            .join("events.jsonl")
            .exists()
    );
    let (done, tally) = release_raw_with_activity(
        &activity,
        None,
        std::slice::from_ref(&proven),
        RawReleaseClass::Policy,
        DateTime::UNIX_EPOCH,
    );
    assert_eq!(done.targets[0].removed.len(), 1);
    assert_eq!(tally.on_record, 1);
    assert!(
        fixture
            .root
            .join(proven.segment_rel())
            .join("audio.jsonl")
            .exists()
    );
    assert!(
        fs::read_to_string(fixture.root.join(proven.segment_rel()).join("events.jsonl"))
            .unwrap()
            .contains("original_deleted")
    );
}

#[test]
fn raw_reserved_proof_and_carrier_exemption_never_authorize_reserved_paths() {
    let fixture = Fixture::new();
    let target = Fixture::target(".tmp-offload-named", "070000_17");
    let reserved = seed_proven(&fixture, &target, "audio.flac");
    let (held, tally) = release_raw(
        &fixture.root,
        std::slice::from_ref(&reserved),
        RawReleaseClass::Policy,
        DateTime::UNIX_EPOCH,
    );
    assert!(held.targets[0].removed.is_empty());
    assert_eq!(tally.on_record, 0);
    assert!(fixture.root.join(reserved.rel()).exists());

    let clean = Fixture::new();
    let activity = clean.activity();
    let carrier = RootActivityCarrier::prepare(Arc::clone(&activity)).unwrap();
    let ancestor = Fixture::target("_default", "../..");
    let ancestor_proof = seed_proven(&clean, &ancestor, "audio.flac");
    assert!(
        observe_with_carrier(activity.journal_root(), carrier.observation())
            .unwrap()
            .unresolved_entries()
            .next()
            .is_none()
    );
    let (held, _) = release_raw_with_activity(
        &activity,
        Some(&carrier),
        std::slice::from_ref(&ancestor_proof),
        RawReleaseClass::Policy,
        DateTime::UNIX_EPOCH,
    );
    assert!(held.targets[0].removed.is_empty());
    assert!(clean.root.join(ancestor_proof.rel()).exists());
    let ordinary = Fixture::target("audio", "070000_17");
    let ordinary_proof = seed_proven(&clean, &ordinary, "audio.flac");
    let (done, _) = release_raw_with_activity(
        &activity,
        Some(&carrier),
        &[ordinary_proof],
        RawReleaseClass::Policy,
        DateTime::UNIX_EPOCH,
    );
    assert_eq!(done.targets[0].removed.len(), 1);
}

#[test]
fn raw_late_binding_refusal_retains_confirmed_first_deletion_and_tally() {
    let fixture = Fixture::new();
    let target = Fixture::target("audio", "070000_17");
    let first = seed_proven(&fixture, &target, "a.flac");
    let second = seed_proven(&fixture, &target, "b.flac");
    let root = fixture.root.clone();
    let displaced = root.with_extension("displaced");
    let displaced_cleanup = displaced.clone();
    let (outcome, tally) = with_checkpoint(
        move |phase, _| {
            if phase == Phase::AfterRaw {
                fs::rename(&root, &displaced).unwrap();
                fs::create_dir(&root).unwrap();
            }
        },
        || {
            release_raw(
                &fixture.root,
                &[first.clone(), second.clone()],
                RawReleaseClass::Policy,
                DateTime::UNIX_EPOCH,
            )
        },
    );
    assert_eq!(outcome.targets[0].removed.len(), 1);
    assert_eq!(outcome.targets[0].not_removed.len(), 1);
    assert!(outcome.targets[0].post_commit_failure.is_some());
    assert_eq!(tally.on_record, 1);
    assert!(!displaced_cleanup.join(first.rel()).exists());
    assert!(displaced_cleanup.join(second.rel()).exists());
    fs::remove_dir_all(displaced_cleanup).unwrap();
}

#[test]
fn legacy_final_cas_fresh_census_keeps_current_effect_full_marks_and_later_raw() {
    use crate::marks::{Proposal, RemovalClass, preflight, reconcile};
    let fixture = Fixture::new();
    let first = Fixture::target("audio", "070000_17");
    let second = Fixture::target("audio", "080000_17");
    seed_proven(&fixture, &first, "audio.flac");
    seed_proven(&fixture, &second, "audio.flac");
    let proposal = Proposal {
        bytes: 3,
        reason: "fixture".into(),
        names: vec!["audio.flac".into()],
    };
    let register = reconcile(
        &fixture.root,
        RemovalClass::PolicyRawRelease,
        &[(first, proposal.clone()), (second, proposal)],
        crate::marks::mark_at("first"),
    )
    .unwrap();
    let ids = register.marks.keys().cloned().collect::<Vec<_>>();
    let marks = preflight(&fixture.root, &ids).unwrap();
    let before = fs::read(fixture.root.join("health/retention-marks.json")).unwrap();
    let mut errors = Vec::new();
    let policy = crate::policy::policy_from_retention(
        serde_json::json!({"raw_media":"keep"}).as_object().unwrap(),
    );
    let busy = fixture.activity();
    let held = crate::remove_marked::remove_marked(
        &fixture.root,
        &marks,
        &policy,
        chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
        DateTime::UNIX_EPOCH,
        &mut errors,
    );
    assert!(held.targets.is_empty());
    assert!(held.halted.is_some());
    assert_eq!(
        fs::read(fixture.root.join("health/retention-marks.json")).unwrap(),
        before
    );
    for (_, mark) in marks.as_slice() {
        assert!(
            fixture
                .root
                .join(segment_rel(&mark.target))
                .join("audio.flac")
                .exists()
        );
    }
    drop(busy);
    let root = fixture.root.clone();
    let outcome = with_checkpoint(
        move |phase, _| {
            if phase == Phase::BeforeRawCas {
                fs::create_dir(root.join(".tmp-offload-new-before-cas")).unwrap();
            }
        },
        || {
            crate::remove_marked::remove_marked(
                &fixture.root,
                &marks,
                &policy,
                chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap(),
                DateTime::UNIX_EPOCH,
                &mut errors,
            )
        },
    );
    assert_eq!(outcome.targets.len(), 1);
    assert_eq!(outcome.targets[0].removed.len(), 1);
    assert!(outcome.halted.is_some());
    assert!(!errors.is_empty());
    assert_eq!(
        fs::read(fixture.root.join("health/retention-marks.json")).unwrap(),
        before
    );
    let later = &marks.as_slice()[1].1.target;
    assert!(
        fixture
            .root
            .join(segment_rel(later))
            .join("audio.flac")
            .exists()
    );
}

fn actual_stage(activity: &Arc<ActivityLease>, relative: &str) -> MediaStage {
    let parent =
        MediaDirectory::open(activity.journal_root().clone(), Path::new(relative)).unwrap();
    MediaStage::create(parent, activity.clone())
        .unwrap_or_else(|_| panic!("stage preparation failed"))
}

#[test]
fn direct_reserved_and_parent_requests_preserve_every_kind() {
    for stream in ["_default", "field.audio"] {
        for kind in ["empty", "occupied", "file"] {
            let fixture = Fixture::new();
            let target = Fixture::target(stream, ".tmp-offload-reserved");
            let path = fixture.root.join(segment_rel(&target));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            match kind {
                "file" => fs::write(&path, b"reserved file").unwrap(),
                "occupied" => {
                    fs::create_dir(&path).unwrap();
                    fs::write(path.join("payload"), b"sole raw").unwrap();
                }
                _ => fs::create_dir(&path).unwrap(),
            }
            let direct = remove_at(&fixture, &target);
            assert!(direct.targets[0].removed.is_empty());
            assert!(!direct.targets[0].not_removed.is_empty());
            // An empty stage in a restore location holds no data, so deletion's
            // admission retires it; the reserved request itself is still refused.
            assert_eq!(path.exists(), kind != "empty");
            if kind == "empty" {
                // Nothing reserved remains for the parent requests to protect.
                continue;
            }
            if kind == "occupied" {
                assert_eq!(fs::read(path.join("payload")).unwrap(), b"sole raw");
            }
            if stream != "_default" {
                let parent = Fixture::target("_default", stream);
                let outcome = remove_at(&fixture, &parent);
                assert!(outcome.targets[0].removed.is_empty());
                assert!(!outcome.targets[0].not_removed.is_empty());
                assert!(path.exists());
                assert!(
                    !fixture
                        .root
                        .join(format!("chronicle/20261011/.removing_{stream}"))
                        .exists()
                );
            } else {
                let parent = Fixture::target("_default", ".");
                let outcome = remove_at(&fixture, &parent);
                assert!(outcome.targets[0].removed.is_empty());
                assert!(!outcome.targets[0].not_removed.is_empty());
                assert!(path.exists());
            }
            assert!(!path.join(TOMBSTONE_NAME).exists());
        }
    }
}

#[test]
fn wrapped_reservations_refuse_direct_and_both_recovery_entries() {
    for wraps in [1, 2, 3] {
        let fixture = Fixture::new();
        let name = format!("{}.tmp-offload-hidden", ".removing_".repeat(wraps));
        let target = Fixture::target("field.audio", &name);
        let path = fixture.populate(&target);
        assert!(remove_at(&fixture, &target).targets[0].removed.is_empty());
        assert_eq!(
            fs::read(path.join("audio.flac")).unwrap(),
            b"sole raw bytes"
        );
        let original =
            Fixture::target("field.audio", crate::staging::original_name(&name).unwrap());
        let row = recover_segment(
            &fixture.root,
            &original,
            "now",
            RemovalReason::OwnerSegmentDelete,
            "fixture",
        )
        .unwrap();
        assert!(row.removed.is_empty());
        assert!(!row.not_removed.is_empty());
        let all = recover(
            &fixture.root,
            "now",
            RemovalReason::OwnerSegmentDelete,
            "fixture",
        );
        assert!(all.targets.iter().all(|row| row.removed.is_empty()));
        assert_eq!(
            fs::read(path.join("audio.flac")).unwrap(),
            b"sole raw bytes"
        );
        assert!(!path.join(TOMBSTONE_NAME).exists());
    }
    let fixture = Fixture::new();
    let ordinary = Fixture::target("field.audio", ".removing_.removing_070000_17_summary");
    let path = fixture.populate(&ordinary);
    let outcome = remove_at(&fixture, &ordinary);
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    assert!(path.join(TOMBSTONE_NAME).exists());
}

#[test]
fn actual_named_staged_parent_is_seen_by_sole_walk_and_both_recoveries_hold() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("chronicle/20261011/field.audio")).unwrap();
    let activity = fixture.activity();
    let stage = actual_stage(&activity, "chronicle/20261011/field.audio");
    fs::write(stage.diagnostic_path().join("payload"), b"sole raw").unwrap();
    let basename = stage.diagnostic_path().file_name().unwrap().to_owned();
    let staged_parent = fixture
        .root
        .join("chronicle/20261011/.removing_field.audio");
    fs::rename(
        fixture.root.join("chronicle/20261011/field.audio"),
        &staged_parent,
    )
    .unwrap();
    let census = observe_obligations(activity.journal_root()).unwrap();
    let expected = Path::new("chronicle/20261011/.removing_field.audio").join(basename);
    assert!(census.entries().iter().any(|item| matches!(item,
        solstone_core_journal_io::activity_obligations::NativeObligation::Stage { relative, .. } if relative == &expected
    )));
    let target = Fixture::target("_default", "field.audio");
    let row = recover_segment_with_activity(
        &activity,
        None,
        &target,
        "now",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    )
    .unwrap();
    assert!(row.removed.is_empty());
    assert!(!row.not_removed.is_empty());
    let all = recover_with_activity(
        &activity,
        None,
        "now",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    );
    assert!(all.halted.is_some());
    assert_eq!(
        fs::read(fixture.root.join(expected).join("payload")).unwrap(),
        b"sole raw"
    );
    assert!(!staged_parent.join(TOMBSTONE_NAME).exists());
}

#[test]
fn original_lease_reuse_succeeds_without_second_admission_and_foreign_carrier_refuses() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17_summary");
    let path = fixture.populate(&target);
    let activity = fixture.activity();
    let carrier = RootActivityCarrier::prepare(activity.clone())
        .unwrap_or_else(|_| panic!("carrier preparation failed"));
    let carrier_name = carrier
        .diagnostic_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for name in [&carrier_name, "."] {
        let carrier_or_ancestor = Target {
            day: "..".into(),
            stream: "_default".into(),
            dir: name.to_owned(),
        };
        let outcome = borrowed_remove(&activity, Some(&carrier), &[carrier_or_ancestor]);
        assert!(outcome.targets[0].removed.is_empty());
        assert!(!outcome.targets[0].not_removed.is_empty());
        assert!(carrier.diagnostic_path().is_dir());
    }
    let independent = remove_at(&fixture, &target);
    assert!(independent.targets[0].removed.is_empty());
    assert_eq!(
        fs::read(path.join("audio.flac")).unwrap(),
        b"sole raw bytes"
    );

    let foreign = Fixture::new();
    let foreign_activity = foreign.activity();
    let foreign_carrier = RootActivityCarrier::prepare(foreign_activity)
        .unwrap_or_else(|_| panic!("foreign carrier failed"));
    let refused = borrowed_remove(
        &activity,
        Some(&foreign_carrier),
        std::slice::from_ref(&target),
    );
    assert!(refused.targets[0].removed.is_empty());
    assert!(path.join("audio.flac").exists());
    let removed = borrowed_remove(&activity, Some(&carrier), &[target]);
    assert!(removed.targets[0].not_removed.is_empty(), "{removed:?}");
    assert_eq!(removed.targets[0].removed.len(), 2);
    assert!(path.join(TOMBSTONE_NAME).exists());
    assert!(carrier.diagnostic_path().is_dir());
}

#[test]
fn obligation_after_pivot_preserves_staged_raw_before_tombstone() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    fixture.populate(&target);
    let activity = fixture.activity();
    let held = Rc::new(RefCell::new(None));
    let store = held.clone();
    let callback_activity = activity.clone();
    let outcome = with_checkpoint(
        move |phase, _| {
            if phase == Phase::AfterPivot {
                *store.borrow_mut() = Some(actual_stage(
                    &callback_activity,
                    "chronicle/20261011/field.audio",
                ));
            }
        },
        || borrowed_remove(&activity, None, std::slice::from_ref(&target)),
    );
    let staged = fixture.root.join(format!(
        "{}/{}",
        parent_rel(&target),
        staged_name(&target.dir)
    ));
    assert!(held.borrow().is_some());
    assert_eq!(
        fs::read(staged.join("audio.flac")).unwrap(),
        b"sole raw bytes"
    );
    assert!(!staged.join(TOMBSTONE_NAME).exists());
    assert!(outcome.targets[0].removed.is_empty());
    assert!(outcome.targets[0].not_removed[0].staged.is_some());
}

/// An exclusive lease this delete took for itself answers once: what a later
/// phase's census could only have tripped on is not the delete's concern. A busy
/// journal is said in its own words, and a stage already there still refuses.
#[test]
fn own_lease_delete_finishes_past_a_new_day_and_refuses_a_stage_or_a_busy_journal() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    fixture.populate(&target);
    let root = fixture.root.clone();
    let outcome = with_checkpoint(
        move |phase, _| {
            if phase == Phase::AfterPivot {
                fs::create_dir_all(root.join("chronicle/20261105/health")).unwrap();
            }
        },
        || remove_at(&fixture, &target),
    );
    assert!(outcome.targets[0].not_removed.is_empty(), "{outcome:?}");
    assert!(!outcome.targets[0].removed.is_empty());

    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    let path = fixture.populate(&target);
    let held = fixture.activity();
    let outcome = remove_at(&fixture, &target);
    assert_eq!(outcome.targets[0].not_removed[0].reason, WHOLE_BUSY);
    assert!(outcome.targets[0].not_removed[0].staged.is_none());
    assert_eq!(
        fs::read(path.join("audio.flac")).unwrap(),
        b"sole raw bytes"
    );
    drop(held);

    let unrecognised = fixture
        .root
        .join("health/offload/.tmp-offload-unrecognised");
    fs::create_dir_all(unrecognised.join("kept")).unwrap();
    let outcome = remove_at(&fixture, &target);
    assert_eq!(
        outcome.targets[0].not_removed[0].reason,
        WHOLE_ACTIVITY_HOLD
    );
    assert_eq!(
        fs::read(path.join("audio.flac")).unwrap(),
        b"sole raw bytes"
    );
}

#[test]
fn final_restore_refusal_preserves_payload_effects_and_earlier_success() {
    let fixture = Fixture::new();
    let first = Fixture::target("field.audio", "070000_17");
    let second = Fixture::target("field.audio", "080000_17");
    let first_path = fixture.populate(&first);
    let second_path = fixture.populate(&second);
    let activity = fixture.activity();
    let held = Rc::new(RefCell::new(None));
    let store = held.clone();
    let callback_activity = activity.clone();
    let outcome = with_checkpoint(
        move |phase, target| {
            if phase == Phase::BeforeRestore && target.dir == "080000_17" {
                *store.borrow_mut() = Some(actual_stage(
                    &callback_activity,
                    "chronicle/20261011/field.audio",
                ));
            }
        },
        || borrowed_remove(&activity, None, &[first, second.clone()]),
    );
    assert_eq!(outcome.targets[0].removed.len(), 2);
    assert!(outcome.targets[0].not_removed.is_empty());
    assert!(first_path.join(TOMBSTONE_NAME).exists());
    assert!(!second_path.exists());
    let staged = fixture.root.join(format!(
        "{}/{}",
        parent_rel(&second),
        staged_name(&second.dir)
    ));
    assert!(staged.join(TOMBSTONE_NAME).exists());
    assert!(!staged.join("audio.flac").exists());
    assert!(!staged.join("audio.jsonl").exists());
    assert!(outcome.targets[1].removed.is_empty());
    assert!(outcome.targets[1].not_removed[0].staged.is_some());
    assert!(held.borrow().is_some());
}

#[test]
fn unknown_population_and_changed_root_refuse_without_target_effects() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    let path = fixture.populate(&target);
    fs::write(fixture.root.join("health"), b"not a directory").unwrap();
    let outcome = remove_at(&fixture, &target);
    assert!(outcome.targets[0].removed.is_empty());
    assert_eq!(
        fs::read(path.join("audio.flac")).unwrap(),
        b"sole raw bytes"
    );
    fs::remove_file(fixture.root.join("health")).unwrap();
    let activity = fixture.activity();
    let moved = fixture.root.with_extension("moved");
    fs::rename(&fixture.root, &moved).unwrap();
    fs::create_dir(&fixture.root).unwrap();
    fs::write(fixture.root.join("replacement"), b"new root").unwrap();
    let outcome = borrowed_remove(&activity, None, &[target]);
    assert!(outcome.targets[0].removed.is_empty());
    assert_eq!(
        fs::read(moved.join("chronicle/20261011/field.audio/070000_17/audio.flac")).unwrap(),
        b"sole raw bytes"
    );
    assert_eq!(
        fs::read(fixture.root.join("replacement")).unwrap(),
        b"new root"
    );
    fs::remove_file(fixture.root.join("replacement")).unwrap();
    fs::remove_dir(&fixture.root).unwrap();
    fs::rename(moved, &fixture.root).unwrap();
}

#[test]
fn marker_guard_retains_removal_facts_and_never_writes_replacement_root() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    fixture.populate(&target);
    let activity = fixture.activity();
    let root_path = fixture.root.clone();
    let moved = fixture.root.with_extension("marker-moved");
    let callback_moved = moved.clone();
    let outcome = with_checkpoint(
        move |phase, _| {
            if phase == Phase::BeforeMarker {
                fs::rename(&root_path, &callback_moved).unwrap();
                fs::create_dir(&root_path).unwrap();
                fs::write(root_path.join("replacement"), b"new root").unwrap();
            }
        },
        || borrowed_remove(&activity, None, &[target]),
    );
    assert_eq!(outcome.targets[0].removed.len(), 2);
    assert!(outcome.targets[0].post_commit_failure.is_some());
    assert!(!fixture.root.join("health").exists());
    assert_eq!(
        fs::read(fixture.root.join("replacement")).unwrap(),
        b"new root"
    );
    fs::remove_file(fixture.root.join("replacement")).unwrap();
    fs::remove_dir(&fixture.root).unwrap();
    fs::rename(moved, &fixture.root).unwrap();
}

#[test]
fn absent_recovery_is_none_but_uncertain_parent_is_a_refusal_without_phantom_day() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    assert!(
        recover_segment(
            &fixture.root,
            &target,
            "now",
            RemovalReason::OwnerSegmentDelete,
            "fixture"
        )
        .is_none()
    );
    assert!(!fixture.root.join("chronicle").exists());
    fs::create_dir_all(fixture.root.join("chronicle/20261011")).unwrap();
    fs::write(
        fixture.root.join("chronicle/20261011/field.audio"),
        b"not a parent",
    )
    .unwrap();
    let row = recover_segment(
        &fixture.root,
        &target,
        "now",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    )
    .unwrap();
    assert!(!row.not_removed.is_empty());
    assert!(row.removed.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn owner_deletion_retires_an_ended_empty_reservation_but_never_an_occupied_one() {
    let fixture = Fixture::new();
    let target = Fixture::target("field.audio", "070000_17");
    let path = fixture.populate(&target);
    let carrier = RootActivityCarrier::prepare(fixture.activity())
        .unwrap_or_else(|_| panic!("carrier preparation failed"));
    let ended = carrier.diagnostic_path();
    // The owner ends without retiring its reservation, as on a crash.
    drop(carrier);
    let removed = remove_at(&fixture, &target);
    assert!(removed.targets[0].not_removed.is_empty(), "{removed:?}");
    assert_eq!(removed.targets[0].removed.len(), 2);
    assert!(path.join(TOMBSTONE_NAME).exists());
    assert!(!ended.exists());

    let second = Fixture::target("field.audio", "080000_17");
    let second_path = fixture.populate(&second);
    let carrier = RootActivityCarrier::prepare(fixture.activity())
        .unwrap_or_else(|_| panic!("carrier preparation failed"));
    let occupied = carrier.diagnostic_path();
    drop(carrier);
    fs::write(occupied.join("payload"), b"unpublished copy").unwrap();
    let held = remove_at(&fixture, &second);
    assert!(held.targets[0].removed.is_empty());
    assert_eq!(
        fs::read(second_path.join("audio.flac")).unwrap(),
        b"sole raw bytes"
    );
    assert_eq!(
        fs::read(occupied.join("payload")).unwrap(),
        b"unpublished copy"
    );
}
