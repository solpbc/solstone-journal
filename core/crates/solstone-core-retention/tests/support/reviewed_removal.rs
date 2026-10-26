// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "isolated real retained execution and race controls"
)]

use super::*;
use crate::mark_mutation::BoundMutationOutcome;
use crate::marks::{MarkState, Proposal, Register};
use crate::review_context::{
    CheckedReviewContext, check_review_context, load_at_root, read_review_context,
};
use chrono::TimeZone;
use serde_json::json;
use solstone_core_journal_io::media::{
    Cleanup, Durability, MediaPrimitive, RemovalEffect, run_with_media_hook,
};
use solstone_core_journal_io::{FlatDirectory, JournalRoot, acquire_existing_parent_lock_bound};
use std::{ffi::OsStr, fs, sync::Arc, time::Duration};

struct Fixture {
    temp: tempfile::TempDir,
    root: Arc<JournalRoot>,
    marks: Vec<Mark>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("config")).unwrap();
        fs::create_dir(temp.path().join("health")).unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let f = Self {
            temp,
            root,
            marks: Vec::new(),
        };
        f.config(json!({"retention":{"raw_media":"days","raw_media_days":1}}));
        f
    }
    fn config(&self, value: serde_json::Value) {
        fs::write(
            self.path("config/journal.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    fn path(&self, relative: &str) -> std::path::PathBuf {
        self.temp.path().join(relative)
    }
    fn target(&self, named: bool) -> crate::Target {
        crate::Target {
            day: "20260701".into(),
            stream: if named { "field.audio" } else { "_default" }.into(),
            dir: "prefix_070000_17_suffix".into(),
        }
    }
    fn raw(&self, target: &crate::Target, name: &str, empty: bool) -> std::path::PathBuf {
        let segment = self.path(&crate::layout::segment_rel(
            &target.day,
            &target.stream,
            &target.dir,
        ));
        fs::create_dir_all(&segment).unwrap();
        fs::create_dir_all(self.path(&crate::layout::day_health_rel(&target.day))).unwrap();
        let path = segment.join(name);
        fs::write(&path, b"raw").unwrap();
        let record = json!({"_solstone_processing":{"schema":"solstone.processing.v1","state":if empty { "empty" } else { "analyzed" },"reason_code":if empty { "no_decodable_audio" } else { "ok" },"handler":"transcribe","input_size":3,"attempted_at":"2026-07-01T00:00:00Z"}});
        let stem = name.rsplit_once('.').unwrap().0;
        fs::write(
            segment.join(format!("{stem}.jsonl")),
            format!(
                "{record}\n{}",
                if empty {
                    ""
                } else {
                    "{\"start\":0,\"text\":\"x\"}\n"
                }
            ),
        )
        .unwrap();
        path
    }
    fn mark(&mut self, target: crate::Target, names: &[&str], class: RemovalClass) -> Mark {
        let names = names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mark = Mark {
            id: MarkId::derive(class, &target, &names),
            target,
            class,
            marked_at: "first".into(),
            proposal: Proposal {
                names,
                bytes: 3,
                reason: "release".into(),
            },
            state: MarkState::Marked,
            offload: None,
        };
        self.marks.push(mark.clone());
        self.save();
        mark
    }
    fn bound(&mut self, target: crate::Target, name: &str) -> Mark {
        let names = vec![name.to_owned()];
        let proof = crate::archive_proof::OffloadProof {
            journal_id: self.root.identity().observation_id(),
            manifest_id: "manifest".into(),
            target: target.clone(),
            layout: solstone_core_journal_io::paths::SegmentLayout::Direct,
            repository_id: "a".repeat(64),
            snapshot_id: "b".repeat(64),
            files: vec![crate::archive_proof::ArchiveFileProof {
                witness_id: "witness".into(),
                name: name.to_owned(),
                bytes: 3,
                sha256: "c".repeat(64),
                archive_path: format!("/stage/{name}"),
                processing: crate::archive_proof::AcceptedProcessingObservation {
                    record: None,
                    has_analysis_row: true,
                },
            }],
        };
        let mark = Mark {
            id: MarkId::derive(RemovalClass::OffloadRawRelease, &target, &names),
            target,
            class: RemovalClass::OffloadRawRelease,
            marked_at: "first".into(),
            proposal: Proposal {
                names,
                bytes: 3,
                reason: "release".into(),
            },
            state: MarkState::Marked,
            offload: Some(crate::archive_proof::OffloadOffer::new(proof).unwrap()),
        };
        self.marks.push(mark.clone());
        self.save();
        mark
    }
    fn save(&self) {
        let mut r = Register::empty();
        for m in &self.marks {
            if m.offload.is_some() {
                r.version = 2;
            }
            r.marks.insert(m.id.clone(), m.clone());
        }
        fs::write(
            self.path("health/retention-marks.json"),
            serde_json::to_vec(&r).unwrap(),
        )
        .unwrap();
    }
    fn checked(&self, selected: &[Mark]) -> CheckedReviewContext {
        let ids = selected.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        let ctx = read_review_context(&self.root, &ids).unwrap();
        check_review_context(Arc::clone(&self.root), &ctx).unwrap()
    }
    fn run(&self, checked: CheckedReviewContext) -> ReviewedRemoval {
        remove_reviewed(checked, NaiveDate::from_ymd_opt(2026, 8, 6).unwrap(), now())
    }
    fn with(
        &self,
        checked: CheckedReviewContext,
        hook: impl FnMut(ReviewedCheckpoint, &Path),
    ) -> ReviewedRemoval {
        let mut execution = ReviewedExecution {
            outcome: Outcome {
                targets: Vec::new(),
                halted: None,
            },
            refusal: None,
            carrier: None,
            carrier_failure: None,
            attempts: Vec::new(),
            stage_failures: Vec::new(),
            mutation: None,
        };
        if let Err(error) = execute_reviewed_with(
            &checked,
            NaiveDate::from_ymd_opt(2026, 8, 6).unwrap(),
            now(),
            &mut execution,
            hook,
        ) {
            execution.refusal = Some(error);
        }
        ReviewedRemoval { checked, execution }
    }
}
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap()
}

#[test]
fn ordinary_finish_preserves_actual_receipts_and_releases_activity() {
    use solstone_core_journal_io::activity::{ActivityLease, TeardownLease};
    use solstone_core_journal_io::media::{
        ActivityCarrierRetirementEffect, Confirmation, Durability,
    };
    fn send_static<T: Send + 'static>() {}
    send_static::<ReviewedRemovalFinish>();
    send_static::<OwnedReviewedRemoval>();
    send_static::<ReviewedRemovalReport>();

    for named in [false, true] {
        let mut f = Fixture::new();
        let target = f.target(named);
        let raw = f.raw(&target, "audio.flac", false);
        let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
        let result = f.run(f.checked(std::slice::from_ref(&mark)));
        let original_identity = result.attempts()[0].native().receipt().claimed_identity;
        let ReviewedRemovalFinish::Settled(report) = result.finish() else {
            panic!("complete local removal must settle");
        };
        assert!(!raw.exists());
        assert_eq!(report.selected_marks(), &[mark]);
        assert_eq!(report.outcome().removed_paths().count(), 1);
        assert_eq!(report.attempts().len(), 1);
        let attempt = &report.attempts()[0];
        assert_eq!(attempt.native().effect, RemovalEffect::Deleted);
        assert_eq!(attempt.native().cleanup, Cleanup::Complete);
        assert_eq!(attempt.native().claimed_identity, original_identity);
        assert!(attempt.native().failures.is_empty());
        assert!(attempt.append().unwrap().failures().is_empty());
        assert!(attempt.marker().unwrap().is_ok());
        assert!(attempt.binding_error().is_none());
        assert!(matches!(
            report.mutation(),
            Some(Ok(BoundMutationOutcome::Published(_)))
        ));
        let retirement = report.retirement().unwrap();
        assert_eq!(
            retirement.effect(),
            ActivityCarrierRetirementEffect::Removed
        );
        assert_eq!(retirement.root_durability(), Durability::Synced);
        assert_eq!(retirement.confirmation(), Confirmation::Confirmed);
        assert!(retirement.failures().is_empty());
        assert!(report.refusal().is_none());
        assert!(load_at_root(&f.root).unwrap().marks.is_empty());
        assert!(
            solstone_core_journal_io::activity_obligations::observe_obligations(&f.root)
                .unwrap()
                .entries()
                .is_empty()
        );
        {
            let next = ActivityLease::try_acquire(Arc::clone(&f.root)).unwrap();
            next.revalidate().unwrap();
        }
        let next = TeardownLease::try_acquire(Arc::clone(&f.root)).unwrap();
        next.revalidate().unwrap();
        assert_eq!(report.attempts()[0].native().effect, RemovalEffect::Deleted);
    }
}

#[test]
fn ordinary_direct_hold_of_a_kept_partial_preserves_owners() {
    use solstone_core_journal_io::activity::{ActivityAdmissionError, TeardownLease};
    for partial in [false, true] {
        let mut f = Fixture::new();
        let target = f.target(false);
        let raw = f.raw(&target, "ordinary.flac", false);
        let empty = partial.then(|| f.raw(&target, "empty.flac", true));
        f.config(json!({"retention":{"raw_media":"keep"}}));
        let names = if partial {
            vec!["empty.flac", "ordinary.flac"]
        } else {
            vec!["ordinary.flac"]
        };
        let mark = f.mark(target, &names, RemovalClass::PolicyRawRelease);
        let result = f.run(f.checked(std::slice::from_ref(&mark)));
        let held = Box::new(result.into_owned_hold());
        assert_eq!(held.removal().selected_marks(), std::slice::from_ref(&mark));
        assert_eq!(
            held.removal().outcome().removed_paths().count(),
            usize::from(partial)
        );
        assert_eq!(held.removal().attempts().len(), usize::from(partial));
        if partial {
            assert_eq!(
                held.removal().attempts()[0].native().receipt().effect,
                RemovalEffect::Deleted
            );
        }
        assert!(held.retirement().is_none());
        assert!(held.removal().carrier_preparation().is_some());
        assert!(raw.exists());
        assert!(empty.is_none_or(|path| !path.exists()));
        assert_eq!(
            load_at_root(&f.root).unwrap().marks.get(&mark.id),
            Some(&mark)
        );
        assert!(matches!(
            TeardownLease::try_acquire(Arc::clone(&f.root)),
            Err(ActivityAdmissionError::Busy)
        ));
    }
}

#[test]
fn ordinary_finish_late_obligation_keeps_completed_effects_and_exclusion() {
    use solstone_core_journal_io::activity::{ActivityAdmissionError, TeardownLease};
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    let late = f.path(".tmp-offload-late");
    fs::create_dir(&late).unwrap();
    let ReviewedRemovalFinish::Held(held) = result.finish() else {
        panic!("late obligation must remain held");
    };
    assert!(matches!(
        held.finish_refusal(),
        Some(ReviewedRemovalFinishRefusal::Native(
            ReviewedArchiveFinishRefusal::Obligations(_)
        ))
    ));
    assert!(!raw.exists());
    assert!(late.exists());
    assert_eq!(held.removal().selected_marks(), &[mark]);
    assert_eq!(held.removal().outcome().removed_paths().count(), 1);
    assert_eq!(
        held.removal().attempts()[0].native().receipt().effect,
        RemovalEffect::Deleted
    );
    assert_eq!(
        held.removal().attempts()[0].native().receipt().cleanup,
        Cleanup::Complete
    );
    assert!(load_at_root(&f.root).unwrap().marks.is_empty());
    assert!(held.retirement().is_none());
    assert!(held.removal().carrier_preparation().is_some());
    assert!(matches!(
        TeardownLease::try_acquire(Arc::clone(&f.root)),
        Err(ActivityAdmissionError::Busy)
    ));
}

#[test]
fn ordinary_finish_removed_but_unsynced_retirement_retains_os_error_and_owners() {
    use solstone_core_journal_io::activity::{ActivityAdmissionError, TeardownLease};
    use solstone_core_journal_io::media::{
        ActivityCarrierRetirementEffect, Confirmation, Durability,
    };
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    let (finish, consumed) = run_with_media_hook(
        MediaPrimitive::CarrierRetirementRootSync,
        || Err(std::io::Error::from_raw_os_error(13)),
        || result.finish(),
    );
    assert!(consumed);
    let ReviewedRemovalFinish::Held(held) = finish else {
        panic!("unsynced retirement must remain held");
    };
    assert!(matches!(
        held.finish_refusal(),
        Some(ReviewedRemovalFinishRefusal::Retirement)
    ));
    assert!(!raw.exists());
    assert_eq!(held.removal().selected_marks(), &[mark]);
    assert_eq!(
        held.removal().attempts()[0].native().receipt().effect,
        RemovalEffect::Deleted
    );
    assert_eq!(
        held.removal().attempts()[0].native().receipt().cleanup,
        Cleanup::Complete
    );
    assert!(load_at_root(&f.root).unwrap().marks.is_empty());
    let retirement = held.retirement().unwrap();
    assert_eq!(
        retirement.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(retirement.root_durability(), Durability::Unproven);
    assert_eq!(retirement.confirmation(), Confirmation::Confirmed);
    assert_eq!(retirement.failures().len(), 1);
    assert_eq!(retirement.failures()[0].raw_os_error(), Some(13));
    assert!(held.removal().carrier_preparation().is_some());
    assert!(matches!(
        TeardownLease::try_acquire(Arc::clone(&f.root)),
        Err(ActivityAdmissionError::Busy)
    ));
}

#[test]
fn current_config_age_groups_and_layouts_govern() {
    for named in [false, true] {
        for (raw_rule, preserve, empty, expected) in [
            ("days", false, false, true),
            ("keep", false, false, false),
            ("keep", false, true, true),
            ("days", true, true, false),
        ] {
            let mut f = Fixture::new();
            let target = f.target(named);
            let raw = f.raw(&target, "audio.flac", empty);
            f.config(json!({"retention":{"raw_media":raw_rule,"raw_media_days":1},"transcribe":{"preserve_all":preserve}}));
            let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
            let result = f.run(f.checked(&[mark]));
            assert!(result.refusal().is_none(), "{:?}", result.refusal());
            assert_eq!(!raw.exists(), expected);
            assert_eq!(
                result.outcome().removed_paths().count(),
                usize::from(expected)
            );
            assert_eq!(load_at_root(&f.root).unwrap().marks.is_empty(), expected);
            assert!(result.carrier_preparation().is_some());
        }
    }
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
    let result = remove_reviewed(
        f.checked(&[mark]),
        NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).single().unwrap(),
    );
    assert!(raw.exists());
    assert_eq!(result.outcome().removed_paths().count(), 0);
}

#[test]
fn whole_selection_refuses_stale_unsupported_coordinates_and_metadata() {
    for bad in 0..6 {
        let mut f = Fixture::new();
        let target = f.target(false);
        let raw = f.raw(&target, "audio.flac", false);
        let first = f.mark(
            target.clone(),
            &["audio.flac"],
            RemovalClass::PolicyRawRelease,
        );
        let mut second = f.mark(target, &["b.flac"], RemovalClass::PolicyRawRelease);
        match bad {
            0 => {}
            1 => second.target.dir = "a/b".into(),
            2 => second.target.stream = "a/b".into(),
            3 => second.proposal.names = vec!["events.jsonl".into()],
            4 => second.class = RemovalClass::OwnerSegmentRemoval,
            _ => {
                second.class = RemovalClass::OffloadRawRelease;
                second.offload = Some(
                    crate::archive_proof::OffloadOffer::new(crate::archive_proof::OffloadProof {
                        journal_id: f.root.identity().observation_id(),
                        manifest_id: "manifest".into(),
                        target: second.target.clone(),
                        layout: solstone_core_journal_io::paths::SegmentLayout::Direct,
                        repository_id: "a".repeat(64),
                        snapshot_id: "b".repeat(64),
                        files: vec![crate::archive_proof::ArchiveFileProof {
                            witness_id: "witness".into(),
                            name: "b.flac".into(),
                            bytes: 3,
                            sha256: "c".repeat(64),
                            archive_path: "/stage/b.flac".into(),
                            processing: crate::archive_proof::AcceptedProcessingObservation {
                                record: None,
                                has_analysis_row: true,
                            },
                        }],
                    })
                    .unwrap(),
                );
            }
        }
        second.id = MarkId::derive(second.class, &second.target, &second.proposal.names);
        f.marks[1] = second.clone();
        f.save();
        if bad == 4 {
            let context = read_review_context(&f.root, &[first.id, second.id]).unwrap();
            assert!(check_review_context(Arc::clone(&f.root), &context).is_err());
            assert!(raw.exists());
            continue;
        }
        let checked = f.checked(&[first, second.clone()]);
        if bad == 0 {
            f.marks[1].marked_at = "newer".into();
            f.save();
        }
        let before = fs::read(f.path("health/retention-marks.json")).unwrap();
        let result = f.run(checked);
        assert!(result.refusal().is_some());
        assert!(raw.exists());
        assert!(result.attempts().is_empty());
        assert_eq!(
            fs::read(f.path("health/retention-marks.json")).unwrap(),
            before
        );
    }
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let first = f.mark(
        target.clone(),
        &["audio.flac"],
        RemovalClass::PolicyRawRelease,
    );
    f.mark(target, &["bad.flac"], RemovalClass::OffloadRawRelease);
    f.marks[1].state = MarkState::Failed(Failure {
        at: "before".into(),
        reason: "failed".into(),
        staged: None,
    });
    f.save();
    let result = f.run(f.checked(&[first]));
    assert!(!raw.exists());
    assert!(result.refusal().is_none());
    assert_eq!(load_at_root(&f.root).unwrap().marks.len(), 1);
}

#[test]
fn overlapping_rows_attempt_once_and_share_settlement() {
    for reverse in [false, true] {
        for unsettled in [false, true] {
            let mut f = Fixture::new();
            let target = f.target(false);
            f.raw(&target, "audio.flac", false);
            let a = f.mark(
                target.clone(),
                &["audio.flac"],
                RemovalClass::PolicyRawRelease,
            );
            let b = f.mark(
                target,
                &["audio.flac", "gone.flac"],
                RemovalClass::PolicyRawRelease,
            );
            let selected = if reverse { vec![b, a] } else { vec![a, b] };
            if unsettled {
                fs::create_dir(f.path(&crate::layout::content_rel(
                    &selected[0].target.day,
                    &selected[0].target.stream,
                    &selected[0].target.dir,
                    "events.jsonl",
                )))
                .unwrap();
            }
            let result = f.run(f.checked(&selected));
            assert_eq!(result.attempts().len(), 1);
            assert_eq!(result.outcome().removed_paths().count(), 1);
            assert_eq!(result.outcome().targets[0].removed.len(), 1);
            assert!(result.outcome().targets[1].removed.is_empty());
            assert_eq!(result.attempts()[0].class(), selected[0].class);
            let stored = load_at_root(&f.root).unwrap();
            if unsettled {
                assert_eq!(stored.marks.len(), 2);
                assert!(
                    stored
                        .marks
                        .values()
                        .all(|m| matches!(m.state, MarkState::Failed(_)))
                );
                assert!(
                    result
                        .outcome()
                        .targets
                        .iter()
                        .all(|r| r.post_commit_failure.is_some())
                );
            } else {
                assert!(stored.marks.is_empty());
                let event: serde_json::Value = serde_json::from_str(
                    &fs::read_to_string(f.path(&crate::layout::content_rel(
                        &selected[0].target.day,
                        &selected[0].target.stream,
                        &selected[0].target.dir,
                        "events.jsonl",
                    )))
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(event["class"], "policy_raw_release");
            }
        }
    }
}

#[test]
fn native_claim_and_sync_uncertainty_preserve_owners_and_exact_failures() {
    for primitive in [
        MediaPrimitive::AfterRemovalClaim,
        MediaPrimitive::RemovalSourceSync,
        MediaPrimitive::StageRemove,
    ] {
        let mut f = Fixture::new();
        let target = f.target(false);
        let raw = f.raw(&target, "a.flac", false);
        let later = f.raw(&target, "b.flac", false);
        let mark = f.mark(
            target,
            &["a.flac", "b.flac"],
            RemovalClass::PolicyRawRelease,
        );
        let (result, fired) = run_with_media_hook(
            primitive,
            || Err(std::io::Error::other("injected")),
            || f.run(f.checked(std::slice::from_ref(&mark))),
        );
        assert!(fired);
        assert!(!raw.exists());
        let first = &result.attempts()[0];
        if primitive == MediaPrimitive::AfterRemovalClaim {
            assert!(later.exists());
            assert_eq!(result.attempts().len(), 1);
            assert_eq!(first.native().receipt().effect, RemovalEffect::Claimed);
            assert_eq!(result.outcome().removed_paths().count(), 0);
            assert!(first.native().stage_observation().is_some());
        } else {
            assert!(result.outcome().removed_paths().count() > 0);
            assert_eq!(first.native().receipt().effect, RemovalEffect::Deleted);
        }
        assert!(matches!(
            load_at_root(&f.root).unwrap().marks[&mark.id].state,
            MarkState::Failed(_)
        ));
        if primitive == MediaPrimitive::StageRemove {
            assert_eq!(first.native().receipt().cleanup, Cleanup::Pending);
            assert_eq!(
                first.native().receipt().source_durability,
                Durability::Synced
            );
            assert_eq!(
                first.native().receipt().claim_durability,
                Durability::Synced
            );
            assert!(result.outcome().targets[0].post_commit_failure.is_none());
            assert!(later.exists());
            let MarkState::Failed(failure) = &load_at_root(&f.root).unwrap().marks[&mark.id].state
            else {
                panic!("mark not failed");
            };
            let stage_path = failure.staged.as_ref().expect("stage remembered");
            assert!(f.path(stage_path).exists());
            fs::write(&raw, b"newer").unwrap();
            drop(result);
            let activity =
                solstone_core_journal_io::activity::ActivityLease::try_acquire(Arc::clone(&f.root))
                    .unwrap();
            let recovered = recover_interrupted_removals(&activity).expect("local recovery");
            assert!(recovered.is_empty());
            crate::restore_intent::recover_abandoned_restores(&activity).expect("local recovery");
            solstone_core_journal_io::media::retire_orphan_root_carriers(&activity)
                .expect("retire");
            assert_eq!(fs::read(&raw).unwrap(), b"newer");
            assert!(!f.path(stage_path).exists());
            assert!(load_at_root(&f.root).unwrap().marks.contains_key(&mark.id));
        }
    }
}

#[test]
fn actual_claim_config_contention_and_native_obligations_refuse() {
    for admission in 0..3 {
        let mut f = Fixture::new();
        let target = f.target(false);
        let raw = f.raw(&target, "audio.flac", false);
        let mark = f.mark(
            target.clone(),
            &["audio.flac"],
            RemovalClass::PolicyRawRelease,
        );
        let checked = f.checked(std::slice::from_ref(&mark));
        let claim = if admission == 0 {
            Some(
                solstone_core_segment::output_attempt::OutputClaims::discover(
                    &f.path(&crate::layout::segment_rel(
                        &target.day,
                        &target.stream,
                        &target.dir,
                    )),
                    LockOptions {
                        timeout: Duration::ZERO,
                        ..LockOptions::default()
                    },
                )
                .unwrap(),
            )
        } else {
            None
        };
        let parent = FlatDirectory::open(&f.root, Path::new("config")).unwrap();
        let guard = if admission == 1 {
            Some(
                acquire_existing_parent_lock_bound(
                    &parent,
                    OsStr::new("journal.json.lock"),
                    Duration::ZERO,
                    Duration::ZERO,
                )
                .unwrap(),
            )
        } else {
            None
        };
        if admission == 2 {
            // An occupied stage is an outstanding obligation; an empty one an
            // ended owner left behind is retired at admission instead.
            fs::create_dir(f.path(".tmp-offload-held")).unwrap();
            fs::write(f.path(".tmp-offload-held/payload"), b"held").unwrap();
        }
        let result = f.run(checked);
        assert!(result.refusal().is_some());
        assert!(raw.exists());
        assert!(result.attempts().is_empty());
        assert!(claim.is_some() || guard.is_some() || admission == 2);
    }
}

#[test]
fn post_hash_and_between_file_drift_stop_before_later_raw() {
    for drift in 0..5 {
        let mut f = Fixture::new();
        let target = f.target(false);
        let a = f.raw(&target, "a.flac", false);
        let b = f.raw(&target, "b.flac", false);
        let mark = f.mark(
            target.clone(),
            &["a.flac", "b.flac"],
            RemovalClass::PolicyRawRelease,
        );
        let checked = f.checked(std::slice::from_ref(&mark));
        let mut fired = false;
        let result = f.with(checked, |point, path| {
            if !fired
                && ((drift < 4 && point == ReviewedCheckpoint::Hashed)
                    || (drift == 4 && point == ReviewedCheckpoint::AfterRaw))
            {
                match drift {
                    0 => fs::write(path, b"bad").unwrap(),
                    1 => fs::write(path.with_extension("jsonl"), b"{}\n").unwrap(),
                    2 => {
                        f.raw(&target, "new.flac", false);
                    }
                    3 => f.config(json!({"retention":{"raw_media":"keep"}})),
                    _ => f.config(json!({"retention":{"raw_media":"keep"}})),
                }
                fired = true;
            }
        });
        assert!(fired);
        assert!(b.exists());
        if drift == 4 {
            assert!(!a.exists());
            assert_eq!(result.outcome().removed_paths().count(), 1);
            assert!(result.outcome().targets[0].post_commit_failure.is_some());
            assert!(result.mutation().is_none());
        } else {
            assert!(a.exists());
            assert_eq!(result.attempts().len(), 0);
        }
    }
}

#[test]
fn register_handoff_preserves_unrelated_updates_and_refuses_selected_drift() {
    for change in 0..3 {
        let mut f = Fixture::new();
        let target = f.target(false);
        let raw = f.raw(&target, "audio.flac", false);
        let mark = f.mark(
            target.clone(),
            &["audio.flac"],
            RemovalClass::PolicyRawRelease,
        );
        let checked = f.checked(std::slice::from_ref(&mark));
        let mut fired = false;
        let result = f.with(checked, |point, _| {
            if point == ReviewedCheckpoint::BeforeBookkeeping {
                // Real acquisition proves the raw-phase register guard has ended.
                let health = FlatDirectory::open(&f.root, Path::new("health")).unwrap();
                let _guard = acquire_existing_parent_lock_bound(
                    &health,
                    OsStr::new("retention-marks.json.lock"),
                    Duration::ZERO,
                    Duration::ZERO,
                )
                .unwrap();
                let mut register = load_at_root(&f.root).unwrap();
                if change == 0 {
                    let mut other = mark.clone();
                    other.proposal.names = vec!["other.flac".into()];
                    other.id = MarkId::derive(other.class, &other.target, &other.proposal.names);
                    register.marks.insert(other.id.clone(), other);
                } else if change == 1 {
                    register.marks.get_mut(&mark.id).unwrap().marked_at = "newer".into();
                } else {
                    fs::rename(
                        f.path("health/retention-marks.json.lock"),
                        f.path("health/retained-old.lock"),
                    )
                    .unwrap();
                    fs::write(f.path("health/retention-marks.json.lock"), b"competitor").unwrap();
                }
                fs::write(
                    f.path("health/retention-marks.json"),
                    serde_json::to_vec(&register).unwrap(),
                )
                .unwrap();
                fired = true;
            }
        });
        assert!(fired);
        assert!(!raw.exists());
        assert_eq!(result.outcome().removed_paths().count(), 1);
        let register = load_at_root(&f.root).unwrap();
        if change == 0 {
            assert_eq!(register.marks.len(), 1);
            assert!(!register.marks.contains_key(&mark.id));
            assert!(matches!(
                result.mutation(),
                Some(Ok(BoundMutationOutcome::Published(_)))
            ));
        } else {
            assert!(result.mutation().unwrap().is_err());
            assert!(register.marks.contains_key(&mark.id));
            assert!(result.outcome().targets[0].post_commit_failure.is_some());
        }
    }
}

#[test]
fn owning_guard_refuses_prepublication_swap_and_preserves_published_late_swap() {
    use solstone_core_journal_io::atomic::{
        BoundPublicationPrimitive, run_with_bound_publication_barrier,
    };
    for after in [false, true] {
        let mut f = Fixture::new();
        let target = f.target(false);
        f.raw(&target, "audio.flac", false);
        let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
        let checked = f.checked(std::slice::from_ref(&mark));
        if !after {
            let guard = crate::mark_mutation::CheckedMarkGuard::acquire(&checked).unwrap();
            fs::rename(
                f.path("health/retention-marks.json.lock"),
                f.path("health/old.lock"),
            )
            .unwrap();
            fs::write(f.path("health/retention-marks.json.lock"), b"new").unwrap();
            assert!(
                guard
                    .publish(&[PlannedMarkMutation::resolve(&mark).unwrap()])
                    .is_err()
            );
            assert!(load_at_root(&f.root).unwrap().marks.contains_key(&mark.id));
        } else {
            let path = f.temp.path().to_owned();
            let (outcome, fired) = run_with_bound_publication_barrier(
                BoundPublicationPrimitive::ParentSync,
                1,
                move || {
                    fs::rename(
                        path.join("health/retention-marks.json.lock"),
                        path.join("health/old.lock"),
                    )
                    .unwrap();
                    fs::write(path.join("health/retention-marks.json.lock"), b"new").unwrap();
                },
                || crate::mark_mutation::decline_reviewed(checked),
            );
            assert!(fired);
            let BoundMutationOutcome::PublishedUncertain {
                candidate,
                uncertainty,
            } = outcome.unwrap()
            else {
                panic!("publication lost")
            };
            assert!(candidate.register.marks.is_empty());
            assert!(uncertainty.lock.is_some());
        }
    }
}

#[test]
fn legacy_unbound_offload_rows_refuse_the_whole_selection() {
    for case in ["alone", "policy", "kept_empty"] {
        let mut f = Fixture::new();
        let target = f.target(false);
        let empty = f.raw(&target, "a.flac", true);
        let ordinary = f.raw(&target, "b.flac", false);
        if case == "kept_empty" {
            f.config(json!({"retention":{"raw_media":"keep"}}));
        }
        let policy = f.mark(target.clone(), &["b.flac"], RemovalClass::PolicyRawRelease);
        let legacy = f.mark(
            target,
            &["a.flac", "b.flac"],
            RemovalClass::OffloadRawRelease,
        );
        let selected = if case == "policy" {
            vec![policy, legacy]
        } else {
            vec![legacy]
        };
        let before = fs::read(f.path("health/retention-marks.json")).unwrap();
        let result = f.run(f.checked(&selected));
        assert!(
            matches!(
                result.refusal(),
                Some(ReviewedRemovalRefusal::InvalidSelection)
            ),
            "{case}: {:?}",
            result.refusal()
        );
        assert!(result.attempts().is_empty());
        assert!(empty.exists() && ordinary.exists(), "{case}");
        assert_eq!(
            fs::read(f.path("health/retention-marks.json")).unwrap(),
            before
        );
    }
    // The archive path shares this predicate: a bound offer does not admit an
    // unbound offload row riding along with it.
    let mut f = Fixture::new();
    let target = f.target(false);
    let bound = f.bound(target.clone(), "c.flac");
    let policy = f.mark(target.clone(), &["b.flac"], RemovalClass::PolicyRawRelease);
    let mut legacy = bound.clone();
    legacy.offload = None;
    legacy.proposal.names = vec!["a.flac".into()];
    legacy.id = MarkId::derive(legacy.class, &legacy.target, &legacy.proposal.names);
    assert!(valid_reviewed_coordinates(&[bound.clone(), policy.clone()]));
    assert!(!valid_reviewed_coordinates(&[bound, policy, legacy]));
}

#[test]
fn policy_only_refuses_an_owner_deleted_segment() {
    for named in [false, true] {
        let mut f = Fixture::new();
        let target = f.target(named);
        let raw = f.raw(&target, "audio.flac", false);
        let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
        let checked = f.checked(std::slice::from_ref(&mark));
        let before = fs::read(f.path("health/retention-marks.json")).unwrap();
        // The owner deletes the segment after the segment claim was checked,
        // immediately before its first original would be released.
        let mut fired = false;
        let result = f.with(checked, |point, path| {
            if point == ReviewedCheckpoint::BeforeRaw && !fired {
                fs::write(path.with_file_name("tombstone.json"), b"{}").unwrap();
                fired = true;
            }
        });
        assert!(fired);
        assert!(result.refusal().is_some());
        assert!(result.attempts().is_empty());
        assert!(raw.exists());
        assert_eq!(result.outcome().removed_paths().count(), 0);
        assert_eq!(
            fs::read(f.path("health/retention-marks.json")).unwrap(),
            before
        );
    }
}

#[test]
fn policy_only_settles_on_a_register_holding_an_unselected_bound_offer() {
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let offered = f.raw(&target, "kept.flac", false);
    let bound = f.bound(target.clone(), "kept.flac");
    let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
    assert_eq!(load_at_root(&f.root).unwrap().version, 2);
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    assert!(result.refusal().is_none(), "{:?}", result.refusal());
    let ReviewedRemovalFinish::Settled(report) = result.finish() else {
        panic!("a complete policy release on a version 2 register must settle");
    };
    assert!(!raw.exists());
    assert!(offered.exists());
    assert_eq!(report.outcome().removed_paths().count(), 1);
    assert!(matches!(
        report.mutation(),
        Some(Ok(BoundMutationOutcome::Published(_)))
    ));
    let register = load_at_root(&f.root).unwrap();
    assert_eq!(register.version, 2);
    assert_eq!(register.marks.len(), 1);
    assert_eq!(register.marks.get(&bound.id), Some(&bound));
}

#[cfg(target_os = "linux")]
#[test]
fn policy_only_retires_an_empty_reservation_an_ended_owner_left() {
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
    fs::create_dir(f.path(".tmp-offload-held")).unwrap();
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    assert!(result.refusal().is_none(), "{:?}", result.refusal());
    assert!(!raw.exists());
    assert!(!f.path(".tmp-offload-held").exists());
}

#[test]
fn known_unremoved_files_settle_and_an_uncertain_remnant_holds() {
    use solstone_core_journal_io::activity::TeardownLease;
    // One original is released, one is kept by the current policy and one is
    // already gone. Every outcome is known, so the run settles.
    let mut f = Fixture::new();
    let target = f.target(false);
    let empty = f.raw(&target, "a.flac", true);
    let kept = f.raw(&target, "b.flac", false);
    f.config(json!({"retention":{"raw_media":"keep"}}));
    let mark = f.mark(
        target.clone(),
        &["a.flac", "b.flac", "gone.flac"],
        RemovalClass::PolicyRawRelease,
    );
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    let ReviewedRemovalFinish::Settled(report) = result.finish() else {
        panic!("known unremoved files must not hold the run");
    };
    assert!(!empty.exists());
    assert!(kept.exists());
    assert_eq!(report.outcome().removed_paths().count(), 1);
    let reasons = report.outcome().targets[0]
        .not_removed
        .iter()
        .map(|item| (item.entry.rsplit('/').next().unwrap(), item.reason.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [("b.flac", KEPT_FOREVER), ("gone.flac", NO_LONGER_PRESENT)]
    );
    assert!(report.retirement().unwrap().failures().is_empty());
    assert!(TeardownLease::try_acquire(Arc::clone(&f.root)).is_ok());
    // A refusal before any staging is known too, even without a carrier.
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(
        target.clone(),
        &["audio.flac"],
        RemovalClass::PolicyRawRelease,
    );
    fs::create_dir(f.path(".tmp-offload-held")).unwrap();
    fs::write(f.path(".tmp-offload-held/payload"), b"held").unwrap();
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    let ReviewedRemovalFinish::Settled(report) = result.finish() else {
        panic!("a refusal before staging must not hold the run");
    };
    assert!(matches!(
        report.refusal(),
        Some(ReviewedRemovalRefusal::Obligations(_))
    ));
    assert!(report.retirement().is_none());
    assert!(raw.exists());
    // A deletion whose record could not be written is an owned remnant.
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(
        target.clone(),
        &["audio.flac"],
        RemovalClass::PolicyRawRelease,
    );
    fs::create_dir(raw.with_file_name("events.jsonl")).unwrap();
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    let ReviewedRemovalFinish::Held(held) = result.finish() else {
        panic!("an unrecorded deletion must remain held");
    };
    assert!(matches!(
        held.finish_refusal(),
        Some(ReviewedRemovalFinishRefusal::IncompleteSelection)
    ));
    assert!(!raw.exists());
    assert!(matches!(
        TeardownLease::try_acquire(Arc::clone(&f.root)),
        Err(solstone_core_journal_io::activity::ActivityAdmissionError::Busy)
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn policy_approval_resolves_a_removal_interrupted_mid_move() {
    use solstone_core_journal_io::media::{MediaDirectory, MediaStage, REMOVAL_ORIGINAL_LEAF};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let mut f = Fixture::new();
    let target = f.target(false);
    let raw = f.raw(&target, "audio.flac", false);
    let mark = f.mark(target, &["audio.flac"], RemovalClass::PolicyRawRelease);
    // An earlier approval ended abruptly after moving the original aside.
    let stage = {
        let activity = ActivityLease::try_acquire(Arc::clone(&f.root)).unwrap();
        let segment = raw.parent().unwrap();
        let parent = MediaDirectory::open(
            Arc::clone(&f.root),
            segment
                .parent()
                .unwrap()
                .strip_prefix(f.temp.path())
                .unwrap(),
        )
        .unwrap();
        let source = parent
            .open_child(segment.file_name().unwrap())
            .unwrap()
            .unwrap();
        let expected = source
            .observe_file(OsStr::new("audio.flac"))
            .unwrap()
            .unwrap();
        let stage = MediaStage::create(Arc::clone(&parent), activity).unwrap();
        let path = stage.diagnostic_path();
        let ended = catch_unwind(AssertUnwindSafe(|| {
            run_with_media_hook(
                MediaPrimitive::BeforeClaimDelete,
                || -> std::io::Result<()> { panic!("the process ended here") },
                || source.remove_observed(OsStr::new("audio.flac"), &expected, stage),
            )
        }));
        assert!(ended.is_err());
        path
    };
    assert!(!raw.exists());
    assert!(stage.join(REMOVAL_ORIGINAL_LEAF).is_file());
    // The next approval puts it back first, then removes it as approved.
    let result = f.run(f.checked(std::slice::from_ref(&mark)));
    assert!(result.refusal().is_none(), "{:?}", result.refusal());
    let ReviewedRemovalFinish::Settled(report) = result.finish() else {
        panic!("the resumed approval must settle");
    };
    assert_eq!(report.outcome().removed_paths().count(), 1);
    assert!(!raw.exists());
    assert!(!stage.exists());
    assert!(load_at_root(&f.root).unwrap().marks.is_empty());
}
