// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "test code; the crate-level denials exist to constrain the store"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_journal_io::{LockOptions, hold_lock};
use solstone_core_retention::Target;
use solstone_core_retention::marks::{
    Failure, MarkId, MarkState, Proposal, Register, RemovalClass, StoreError, load, reconcile,
    reconcile_recovered, record_failure, resolve,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Bed {
    path: PathBuf,
}

impl Bed {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "retention-marks-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn target(day: &str, stream: &str, dir: &str) -> Target {
    Target {
        day: day.to_owned(),
        stream: stream.to_owned(),
        dir: dir.to_owned(),
    }
}

fn proposal(reason: &str) -> Proposal {
    Proposal {
        bytes: 42,
        reason: reason.to_owned(),
        names: vec!["a.flac".to_owned(), "b.wav".to_owned()],
    }
}

fn mark_id(class: RemovalClass, target: &Target) -> MarkId {
    MarkId::derive(class, target, &proposal("test").names)
}

fn failure(staged: Option<&str>) -> Failure {
    Failure {
        at: "2026-08-06T12:00:00Z".to_owned(),
        reason: "the staged directory needs recovery".to_owned(),
        staged: staged.map(str::to_owned),
    }
}

#[test]
fn absent_register_is_empty_without_materializing() {
    let bed = Bed::new();
    assert_eq!(load(bed.path()).unwrap(), Register::empty());
    assert!(!bed.path().join("health").exists());
}

#[test]
fn malformed_register_is_not_treated_as_empty() {
    let bed = Bed::new();
    let register = bed.path().join("health").join("retention-marks.json");
    fs::create_dir_all(register.parent().unwrap()).unwrap();
    fs::write(register, b"{not json").unwrap();
    assert!(matches!(load(bed.path()), Err(StoreError::Malformed(_))));
}

#[test]
fn unknown_version_is_rejected() {
    let bed = Bed::new();
    let register = bed.path().join("health").join("retention-marks.json");
    fs::create_dir_all(register.parent().unwrap()).unwrap();
    fs::write(register, b"{\"version\":2,\"marks\":{}}\n").unwrap();
    assert!(matches!(
        load(bed.path()),
        Err(StoreError::UnsupportedVersion { found: 2 })
    ));
}

#[test]
fn unknown_fields_are_rejected() {
    let bed = Bed::new();
    let register = bed.path().join("health").join("retention-marks.json");
    fs::create_dir_all(register.parent().unwrap()).unwrap();
    fs::write(
        register,
        b"{\"version\":1,\"marks\":{},\"unexpected\":true}\n",
    )
    .unwrap();
    assert!(matches!(load(bed.path()), Err(StoreError::Malformed(_))));
}

#[test]
fn inconsistent_map_entries_are_rejected() {
    let bed = Bed::new();
    let target = target("20260805", "field.audio", "070000_17");
    let id = mark_id(RemovalClass::OwnerRawRelease, &target);
    let invalid = json!({
        "version": 1,
        "marks": {
            "not-the-mark-id": {
                "id": id.as_str(),
                "class": "owner_raw_release",
                "target": {"day": target.day, "stream": target.stream, "dir": target.dir},
                "marked_at": "2026-08-06T12:00:00Z",
                "proposal": {"bytes": 42, "reason": "owner requested it", "names": ["a.flac", "b.wav"]},
                "state": "marked"
            }
        }
    });
    let register = bed.path().join("health").join("retention-marks.json");
    fs::create_dir_all(register.parent().unwrap()).unwrap();
    fs::write(register, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(matches!(
        load(bed.path()),
        Err(StoreError::Integrity { .. })
    ));
}

#[test]
fn register_round_trips_marked_and_failed_entries() {
    let bed = Bed::new();
    let marked_target = target("20260805", "field.audio", "070000_17");
    let failed_target = target("20260806", "field.audio", "070100_17");
    let marked = reconcile(
        bed.path(),
        RemovalClass::OwnerRawRelease,
        &[(marked_target.clone(), proposal("owner requested it"))],
        "2026-08-06T12:00:00Z",
    )
    .unwrap();
    let expected = record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &failed_target,
        &proposal("failed").names,
        failure(Some("set-aside/070100_17")),
        "2026-08-06T12:01:00Z",
    )
    .unwrap();
    assert_ne!(marked, expected);
    assert_eq!(load(bed.path()).unwrap(), expected);
}

#[test]
fn reconcile_removes_a_stale_marked_entry() {
    let bed = Bed::new();
    let held = target("20260805", "field.audio", "070000_17");
    reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(held, proposal("old"))],
        "first",
    )
    .unwrap();
    let after = reconcile(bed.path(), RemovalClass::PolicyRawRelease, &[], "second").unwrap();
    assert!(after.marks.is_empty());
}

#[test]
fn reconcile_keeps_a_stale_failed_entry_and_its_staged_path() {
    let bed = Bed::new();
    let held = target("20260805", "field.audio", "070000_17");
    let after_failure = record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &held,
        &proposal("held").names,
        failure(Some("set-aside/070000_17")),
        "first",
    )
    .unwrap();
    let id = mark_id(RemovalClass::PolicyRawRelease, &held);
    let after = reconcile(bed.path(), RemovalClass::PolicyRawRelease, &[], "second").unwrap();
    assert_eq!(after, after_failure);
    assert_eq!(
        after.marks[&id].state,
        MarkState::Failed(failure(Some("set-aside/070000_17")))
    );
}

#[test]
fn reconcile_keeps_a_failed_mark_but_refreshes_its_proposal() {
    let bed = Bed::new();
    let held = target("20260805", "field.audio", "070000_17");
    let id = mark_id(RemovalClass::PolicyRawRelease, &held);
    record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &held,
        &proposal("current").names,
        failure(Some("set-aside/070000_17")),
        "first",
    )
    .unwrap();
    let after = reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(held, proposal("current"))],
        "second",
    )
    .unwrap();
    assert_eq!(after.marks.len(), 1);
    assert_eq!(after.marks[&id].marked_at, "first");
    assert_eq!(after.marks[&id].proposal, proposal("current"));
    assert_eq!(
        after.marks[&id].state,
        MarkState::Failed(failure(Some("set-aside/070000_17")))
    );
}

#[test]
fn reconcile_preserves_marked_at_for_an_existing_marked_entry() {
    let bed = Bed::new();
    let held = target("20260805", "field.audio", "070000_17");
    let first = reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(held.clone(), proposal("current"))],
        "first",
    )
    .unwrap();
    let second = reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(held, proposal("current"))],
        "second",
    )
    .unwrap();
    assert_eq!(first, second);
}

#[test]
fn reconcile_leaves_other_classes_unchanged() {
    let bed = Bed::new();
    let owner = target("20260805", "field.audio", "070000_17");
    let policy = target("20260806", "field.audio", "070100_17");
    let initial = reconcile(
        bed.path(),
        RemovalClass::OwnerRawRelease,
        &[(owner.clone(), proposal("owner"))],
        "first",
    )
    .unwrap();
    let owner_id = mark_id(RemovalClass::OwnerRawRelease, &owner);
    let owner_mark = initial.marks[&owner_id].clone();
    let after = reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(policy, proposal("policy"))],
        "second",
    )
    .unwrap();
    assert_eq!(after.marks[&owner_id], owner_mark);
}

#[test]
fn duplicate_proposals_are_rejected() {
    let bed = Bed::new();
    let held = target("20260805", "field.audio", "070000_17");
    assert!(matches!(
        reconcile(
            bed.path(),
            RemovalClass::PolicyRawRelease,
            &[
                (held.clone(), proposal("first")),
                (held, proposal("second"))
            ],
            "first",
        ),
        Err(StoreError::DuplicateProposal { .. })
    ));
    assert!(!bed.path().join("health").exists());
}

#[test]
fn record_failure_converts_a_marked_entry_and_creates_a_missing_one() {
    let bed = Bed::new();
    let marked_target = target("20260805", "field.audio", "070000_17");
    let missing_target = target("20260806", "field.audio", "070100_17");
    reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(marked_target.clone(), proposal("current"))],
        "first",
    )
    .unwrap();
    let marked_id = mark_id(RemovalClass::PolicyRawRelease, &marked_target);
    let converted = record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &marked_target,
        &proposal("current").names,
        failure(None),
        "second",
    )
    .unwrap();
    assert_eq!(converted.marks[&marked_id].marked_at, "first");
    assert_eq!(
        converted.marks[&marked_id].state,
        MarkState::Failed(failure(None))
    );

    let missing_id = MarkId::derive(RemovalClass::OwnerSegmentRemoval, &missing_target, &[]);
    let created = record_failure(
        bed.path(),
        RemovalClass::OwnerSegmentRemoval,
        &missing_target,
        &Vec::new(),
        failure(Some("set-aside/070100_17")),
        "third",
    )
    .unwrap();
    assert_eq!(created.marks[&missing_id].marked_at, "third");
    assert!(created.marks[&missing_id].proposal.names.is_empty());
    assert_eq!(created.marks[&missing_id].proposal.bytes, 0);
}

#[test]
fn resolve_removes_an_existing_mark_and_persists_the_result() {
    let bed = Bed::new();
    let held = target("20260805", "field.audio", "070000_17");
    let id = mark_id(RemovalClass::PolicyRawRelease, &held);
    reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(held, proposal("current"))],
        "first",
    )
    .unwrap();
    let after = resolve(bed.path(), &id).unwrap();
    assert!(!after.marks.contains_key(&id));
    assert_eq!(load(bed.path()).unwrap(), after);
}

#[test]
fn reconcile_recovered_removes_only_failed_marks_whose_staging_is_gone() {
    let bed = Bed::new();
    let present = target("20260805", "field.audio", "070000_17");
    let gone = target("20260806", "field.audio", "070100_17");
    let present_id = mark_id(RemovalClass::PolicyRawRelease, &present);
    let gone_id = mark_id(RemovalClass::PolicyRawRelease, &gone);
    fs::create_dir_all(bed.path().join("set-aside/present")).unwrap();
    record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &present,
        &proposal("present").names,
        failure(Some("set-aside/present")),
        "first",
    )
    .unwrap();
    record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &gone,
        &proposal("gone").names,
        failure(Some("set-aside/gone")),
        "first",
    )
    .unwrap();

    let after = reconcile_recovered(bed.path()).unwrap();
    assert!(after.marks.contains_key(&present_id));
    assert!(!after.marks.contains_key(&gone_id));
    assert_eq!(load(bed.path()).unwrap(), after);
}

#[test]
fn resolving_a_missing_mark_is_a_non_writing_noop() {
    let bed = Bed::new();
    let missing = mark_id(
        RemovalClass::PolicyRawRelease,
        &target("20260805", "field.audio", "070000_17"),
    );
    let before = load(bed.path()).unwrap();
    let after = resolve(bed.path(), &missing).unwrap();
    assert_eq!(before, Register::empty());
    assert_eq!(after, before);
    assert!(
        !bed.path()
            .join("health")
            .join("retention-marks.json")
            .exists()
    );
}

#[test]
fn mutations_use_the_register_lock() {
    let bed = Bed::new();
    let register = bed.path().join("health").join("retention-marks.json");
    let _held = hold_lock(
        &register,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .unwrap();
    // This intentionally waits out the public API's real default lock timeout.
    let error = reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[(
            target("20260805", "field.audio", "070000_17"),
            proposal("current"),
        )],
        "first",
    )
    .unwrap_err();
    assert!(matches!(error, StoreError::Lock(_)));
}

#[test]
fn every_reconcile_branch_is_exercised_in_one_fixture() {
    let bed = Bed::new();
    let stale_marked = target("20260801", "field.audio", "070000_17");
    let stale_failed = target("20260802", "field.audio", "070000_17");
    let matching_failed = target("20260803", "field.audio", "070000_17");
    let matching_marked = target("20260804", "field.audio", "070000_17");
    let new_target = target("20260805", "field.audio", "070000_17");
    reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[
            (stale_marked.clone(), proposal("stale")),
            (matching_marked.clone(), proposal("old marked")),
        ],
        "first",
    )
    .unwrap();
    record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &stale_failed,
        &proposal("stale-failed").names,
        failure(Some("stale-failed")),
        "first",
    )
    .unwrap();
    record_failure(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &matching_failed,
        &proposal("matching-failed").names,
        failure(Some("matching-failed")),
        "first",
    )
    .unwrap();

    let after = reconcile(
        bed.path(),
        RemovalClass::PolicyRawRelease,
        &[
            (matching_failed.clone(), proposal("new failed")),
            (matching_marked.clone(), proposal("new marked")),
            (new_target.clone(), proposal("new")),
        ],
        "second",
    )
    .unwrap();
    assert!(
        !after
            .marks
            .contains_key(&mark_id(RemovalClass::PolicyRawRelease, &stale_marked))
    );
    assert!(
        after
            .marks
            .contains_key(&mark_id(RemovalClass::PolicyRawRelease, &stale_failed))
    );
    let failed_id = mark_id(RemovalClass::PolicyRawRelease, &matching_failed);
    assert_eq!(after.marks[&failed_id].proposal, proposal("new failed"));
    assert!(matches!(
        after.marks[&failed_id].state,
        MarkState::Failed(_)
    ));
    let marked_id = mark_id(RemovalClass::PolicyRawRelease, &matching_marked);
    assert_eq!(after.marks[&marked_id].marked_at, "first");
    let new_id = mark_id(RemovalClass::PolicyRawRelease, &new_target);
    assert_eq!(after.marks[&new_id].marked_at, "second");
}
