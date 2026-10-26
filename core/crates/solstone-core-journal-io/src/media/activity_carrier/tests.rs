// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::activity::{ActivityAdmissionError, TeardownLease};
use std::fs;

fn fixture() -> (tempfile::TempDir, Arc<ActivityLease>) {
    let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
        Some(root) => tempfile::tempdir_in(root).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let activity = ActivityLease::try_acquire(root).unwrap();
    (temp, activity)
}

#[cfg(any(unix, windows))]
#[test]
fn abandoned_stage_retirement_removes_only_empty_or_its_download() {
    let (temp, activity) = fixture();
    let parent = MediaDirectory::open(activity.journal_root().clone(), Path::new("")).unwrap();
    for (suffix, leaf) in [
        ("empty", None),
        ("download", Some("download")),
        ("other", Some("other")),
    ] {
        let name = format!(".tmp-offload-{suffix}");
        let path = temp.path().join(&name);
        fs::create_dir(&path).unwrap();
        if let Some(leaf) = leaf {
            fs::write(path.join(leaf), b"retained bytes").unwrap();
        }
        let identity = parent
            .open_child(OsStr::new(&name))
            .unwrap()
            .unwrap()
            .identity();
        let result = retire_unrecorded_stage(
            &activity,
            &parent,
            OsStr::new(&name),
            identity,
            OsStr::new("download"),
        );
        if leaf == Some("other") {
            assert!(result.is_err());
            assert_eq!(fs::read(path.join("other")).unwrap(), b"retained bytes");
        } else {
            assert!(result.unwrap());
            assert!(!path.exists());
        }
    }
    assert_eq!(
        crate::activity_obligations::observe_obligations(activity.journal_root())
            .unwrap()
            .entries()
            .len(),
        1
    );
}

fn teardown_fixture() -> (tempfile::TempDir, Arc<TeardownLease>) {
    let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
        Some(root) => tempfile::tempdir_in(root).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let lease = TeardownLease::try_acquire(root).unwrap();
    (temp, lease)
}

/// Windows has no directory flush: there the teardown carrier is confirmed
/// with its durability unproven, before and after retirement.
#[cfg(any(unix, windows))]
#[test]
fn teardown_carrier_keeps_original_role_through_census_and_confirmed_retirement() {
    let flushed = if cfg!(windows) {
        Durability::Unproven
    } else {
        Durability::Synced
    };
    let (_temp, teardown) = teardown_fixture();
    let root = teardown.journal_root().clone();
    let carrier = RootTeardownCarrier::prepare(teardown.clone()).unwrap();
    assert!(Arc::ptr_eq(carrier.teardown(), &teardown));
    assert!(Arc::ptr_eq(carrier.journal_root(), &root));
    assert_eq!(carrier.preparation().stage_durability(), flushed);
    assert_eq!(carrier.preparation().root_durability(), flushed);
    assert_eq!(
        carrier.preparation().confirmation(),
        Confirmation::Confirmed
    );
    {
        let raw = crate::activity_obligations::observe_obligations(&root).unwrap();
        assert_eq!(raw.entries().len(), 1);
        let recognized =
            crate::activity_obligations::observe_with_carrier(&root, carrier.observation())
                .unwrap();
        assert_eq!(recognized.entries(), raw.entries());
        assert_eq!(recognized.unresolved_entries().count(), 0);
    }
    let path = carrier.diagnostic_path();
    drop(teardown);
    let result = carrier.retire_after_settlement();
    assert_eq!(
        result.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(result.receipt.root_durability(), flushed);
    assert_eq!(result.receipt.confirmation(), Confirmation::Confirmed);
    assert!(result.receipt.failures().is_empty());
    assert!(!path.exists());
    assert!(matches!(
        ActivityLease::try_acquire(root.clone()),
        Err(ActivityAdmissionError::Busy)
    ));
    assert!(matches!(
        TeardownLease::try_acquire(root.clone()),
        Err(ActivityAdmissionError::Busy)
    ));
    assert!(
        crate::activity_obligations::observe_with_carrier(&root, result.owned.observation())
            .is_err()
    );
    drop(result);
    TeardownLease::try_acquire(root).unwrap();
}

#[cfg(unix)]
#[test]
fn teardown_preparation_faults_preserve_original_owner_reservation_and_sync_facts() {
    for (cut, stage, root_sync) in [
        (
            MediaPrimitive::StageCreated,
            Durability::Unproven,
            Durability::Unproven,
        ),
        (
            MediaPrimitive::CarrierStageSync,
            Durability::Unproven,
            Durability::Unproven,
        ),
        (
            MediaPrimitive::CarrierRootSync,
            Durability::Synced,
            Durability::Unproven,
        ),
        (
            MediaPrimitive::CarrierConfirmation,
            Durability::Synced,
            Durability::Synced,
        ),
    ] {
        let (_temp, teardown) = teardown_fixture();
        let root = teardown.journal_root().clone();
        let (result, fired) = run_with_media_hook(
            cut,
            || Err(io::Error::other("cut")),
            || RootTeardownCarrier::prepare(teardown.clone()),
        );
        assert!(fired);
        let failed = result.err().unwrap();
        assert!(Arc::ptr_eq(failed.teardown(), &teardown));
        assert!(Arc::ptr_eq(failed.journal_root(), &root));
        assert_eq!(failed.receipt().stage_durability(), stage);
        assert_eq!(failed.receipt().root_durability(), root_sync);
        assert_eq!(failed.receipt().confirmation(), Confirmation::Unverified);
        let path = failed.diagnostic_path().unwrap();
        assert!(path.is_dir());
        drop(teardown);
        assert!(matches!(
            ActivityLease::try_acquire(root.clone()),
            Err(ActivityAdmissionError::Busy)
        ));
        assert!(matches!(
            TeardownLease::try_acquire(root.clone()),
            Err(ActivityAdmissionError::Busy)
        ));
        drop(failed); // Unknown cooperation holds cleanup, including prelaunch failure.
        assert!(path.is_dir());
        TeardownLease::try_acquire(root).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn teardown_failure_and_uncertain_retirement_keep_original_role_and_independent_facts() {
    let (_temp, teardown) = teardown_fixture();
    let root = teardown.journal_root().clone();
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::CarrierRootSync,
        || Err(io::Error::other("preparation")),
        || RootTeardownCarrier::prepare(teardown.clone()),
    );
    assert!(fired);
    let failed = result.err().unwrap();
    let (retired, fired) = run_with_media_hook(
        MediaPrimitive::CarrierRetirementRootSync,
        || Err(io::Error::other("retirement")),
        || failed.retire_after_settlement(),
    );
    assert!(fired);
    assert!(Arc::ptr_eq(retired.owned.teardown(), &teardown));
    assert_eq!(retired.owned.error().to_string(), "preparation");
    assert_eq!(
        retired.owned.receipt().root_durability(),
        Durability::Unproven
    );
    assert_eq!(
        retired.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(retired.receipt.root_durability(), Durability::Unproven);
    assert_eq!(retired.receipt.confirmation(), Confirmation::Confirmed);
    assert_eq!(retired.receipt.failures().len(), 1);
    drop(teardown);
    assert!(matches!(
        ActivityLease::try_acquire(root.clone()),
        Err(ActivityAdmissionError::Busy)
    ));
    drop(retired);
    ActivityLease::try_acquire(root).unwrap();
}

#[cfg(unix)]
#[test]
fn replaced_lock_splits_exclusion_but_old_teardown_recognition_and_retirement_refuse() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, teardown) = teardown_fixture();
    let root = teardown.journal_root().clone();
    let carrier = RootTeardownCarrier::prepare(teardown.clone()).unwrap();
    let path = carrier.diagnostic_path();
    let entry = temp.path().join("backup-activity.lock");
    let old = temp.path().join("old.lock");
    fs::rename(&entry, &old).unwrap();
    fs::write(&entry, b"replacement").unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
    let new = ActivityLease::try_acquire(root.clone()).unwrap();
    new.revalidate().unwrap();
    assert!(
        crate::activity_obligations::observe_with_carrier(&root, carrier.observation()).is_err()
    );
    let result = carrier.retire_after_settlement();
    assert!(Arc::ptr_eq(result.owned.teardown(), &teardown));
    assert_eq!(
        result.receipt.effect(),
        ActivityCarrierRetirementEffect::NotAttempted
    );
    assert!(!result.receipt.failures().is_empty());
    assert!(path.is_dir());
    assert_eq!(fs::read(old).unwrap(), b"");
    assert_eq!(fs::read(entry).unwrap(), b"replacement");
    new.revalidate().unwrap();
}

fn failure(
    result: Result<RootActivityCarrier, ActivityCarrierPreparationFailure>,
) -> ActivityCarrierPreparationFailure {
    result.err().expect("preparation must refuse")
}

#[test]
fn lost_activity_refuses_before_creating_a_reservation() {
    let (temp, activity) = fixture();
    fs::rename(
        temp.path().join("backup-activity.lock"),
        temp.path().join("old.lock"),
    )
    .unwrap();
    let failed = failure(RootActivityCarrier::prepare(activity));
    assert!(failed.diagnostic_path().is_none());
    assert_eq!(failed.receipt().stage_durability(), Durability::Unproven);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn windows_unproven_directory_durability_holds_created_reservation() {
    let (_temp, activity) = fixture();
    let root = activity.journal_root().clone();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    assert_eq!(
        carrier.preparation().stage_durability(),
        Durability::Unproven
    );
    assert_eq!(
        carrier.preparation().root_durability(),
        Durability::Unproven
    );
    assert_eq!(
        carrier.preparation().confirmation(),
        Confirmation::Confirmed
    );
    let path = carrier.diagnostic_path();
    assert!(path.is_dir());
    assert!(matches!(
        TeardownLease::try_acquire(root.clone()),
        Err(ActivityAdmissionError::Busy)
    ));
    drop(carrier);
    assert!(path.is_dir());
    TeardownLease::try_acquire(root).unwrap();
}

#[cfg(unix)]
#[test]
fn preparation_and_owned_retirement_have_independent_facts_and_retain_lease() {
    let (_temp, activity) = fixture();
    let root = activity.journal_root().clone();
    let carrier = RootActivityCarrier::prepare(activity.clone()).unwrap();
    assert!(Arc::ptr_eq(carrier.activity(), &activity));
    assert!(Arc::ptr_eq(carrier.journal_root(), &root));
    assert_eq!(carrier.preparation().stage_durability(), Durability::Synced);
    assert_eq!(carrier.preparation().root_durability(), Durability::Synced);
    assert_eq!(
        carrier.preparation().confirmation(),
        Confirmation::Confirmed
    );
    let path = carrier.diagnostic_path();
    drop(activity);
    let outcome = carrier.retire_after_settlement();
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(outcome.receipt.root_durability(), Durability::Synced);
    assert_eq!(outcome.receipt.confirmation(), Confirmation::Confirmed);
    assert!(outcome.receipt.failures().is_empty());
    assert!(!path.exists());
    assert!(matches!(
        TeardownLease::try_acquire(root.clone()),
        Err(ActivityAdmissionError::Busy)
    ));
    drop(outcome);
    TeardownLease::try_acquire(root).unwrap();
}

#[cfg(unix)]
#[test]
fn preparation_faults_preserve_reservation_and_each_completed_flush() {
    for (cut, stage, root) in [
        (
            MediaPrimitive::StageCreated,
            Durability::Unproven,
            Durability::Unproven,
        ),
        (
            MediaPrimitive::CarrierStageSync,
            Durability::Unproven,
            Durability::Unproven,
        ),
        (
            MediaPrimitive::CarrierRootSync,
            Durability::Synced,
            Durability::Unproven,
        ),
        (
            MediaPrimitive::CarrierConfirmation,
            Durability::Synced,
            Durability::Synced,
        ),
    ] {
        let (_temp, activity) = fixture();
        let root_handle = activity.journal_root().clone();
        let (result, consumed) = run_with_media_hook(
            cut,
            || Err(io::Error::other("cut")),
            || RootActivityCarrier::prepare(activity),
        );
        assert!(consumed);
        let failed = failure(result);
        assert_eq!(failed.receipt().stage_durability(), stage);
        assert_eq!(failed.receipt().root_durability(), root);
        assert_eq!(failed.receipt().confirmation(), Confirmation::Unverified);
        let path = failed.diagnostic_path().unwrap();
        assert!(path.is_dir());
        assert!(matches!(
            TeardownLease::try_acquire(root_handle.clone()),
            Err(ActivityAdmissionError::Busy)
        ));
        drop(failed); // Unknown cooperation: no cleanup method is called.
        assert!(path.is_dir());
        TeardownLease::try_acquire(root_handle).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn known_unlaunched_failure_can_retire_only_its_owned_empty_reservation() {
    let (_temp, activity) = fixture();
    let (result, consumed) = run_with_media_hook(
        MediaPrimitive::CarrierRootSync,
        || Err(io::Error::other("root flush")),
        || RootActivityCarrier::prepare(activity),
    );
    assert!(consumed);
    let failed = failure(result);
    let path = failed.diagnostic_path().unwrap();
    let outcome = failed.retire_after_settlement();
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(outcome.receipt.confirmation(), Confirmation::Confirmed);
    assert!(!path.exists());
    // Preparation error and preparation durability remain independently retained.
    assert_eq!(outcome.owned.error().to_string(), "root flush");
    assert_eq!(
        outcome.owned.receipt().root_durability(),
        Durability::Unproven
    );
}

#[cfg(unix)]
#[test]
fn nonempty_final_confirmation_holds_failure_and_competing_content() {
    let (_temp, activity) = fixture();
    let root_path = activity.journal_root().canonical_path().to_owned();
    let (result, consumed) = run_with_media_hook(
        MediaPrimitive::CarrierConfirmation,
        move || {
            let stage = fs::read_dir(root_path)?
                .map(Result::unwrap)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".tmp-offload-")
                })
                .unwrap();
            fs::write(stage.path().join("competitor"), b"keep")
        },
        || RootActivityCarrier::prepare(activity),
    );
    assert!(consumed);
    let failed = failure(result);
    let path = failed.diagnostic_path().unwrap();
    let outcome = failed.retire_after_settlement();
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::NotAttempted
    );
    assert_eq!(fs::read(path.join("competitor")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn dropping_prepared_carrier_leaves_reservation_after_lease_release() {
    let (_temp, activity) = fixture();
    let root = activity.journal_root().clone();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    let path = carrier.diagnostic_path();
    drop(carrier);
    assert!(path.is_dir());
    TeardownLease::try_acquire(root).unwrap();
}

#[cfg(unix)]
#[test]
fn root_rebind_before_parent_flush_preserves_reservation_and_replacement() {
    let (temp, activity) = fixture();
    let current = temp.path().to_owned();
    let moved = current.with_extension("old");
    let moved_hook = moved.clone();
    let (result, consumed) = run_with_media_hook(
        MediaPrimitive::CarrierRootSync,
        move || {
            fs::rename(&current, &moved_hook)?;
            fs::create_dir(&current)?;
            fs::write(current.join("competitor"), b"keep")
        },
        || RootActivityCarrier::prepare(activity),
    );
    assert!(consumed);
    let failed = failure(result);
    assert_eq!(failed.receipt().root_durability(), Durability::Unproven);
    let outcome = failed.retire_after_settlement();
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::NotAttempted
    );
    assert_eq!(fs::read(temp.path().join("competitor")).unwrap(), b"keep");
    assert!(fs::read_dir(&moved).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-offload-")
    }));
    drop(outcome);
    fs::remove_dir_all(moved).unwrap();
}

#[cfg(unix)]
#[test]
fn flush_failure_does_not_erase_removed_effect_or_absence_confirmation() {
    let (_temp, activity) = fixture();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    let (outcome, consumed) = run_with_media_hook(
        MediaPrimitive::CarrierRetirementRootSync,
        || Err(io::Error::other("root flush")),
        || carrier.retire_after_settlement(),
    );
    assert!(consumed);
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(outcome.receipt.root_durability(), Durability::Unproven);
    assert_eq!(outcome.receipt.confirmation(), Confirmation::Confirmed);
    assert_eq!(outcome.receipt.failures().len(), 1);
}

#[cfg(unix)]
#[test]
fn late_confirmation_failure_keeps_removal_and_sync_facts() {
    let (_temp, activity) = fixture();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    let (outcome, consumed) = run_with_media_hook(
        MediaPrimitive::CarrierRetirementConfirmation,
        || Err(io::Error::other("confirmation")),
        || carrier.retire_after_settlement(),
    );
    assert!(consumed);
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert_eq!(outcome.receipt.root_durability(), Durability::Synced);
    assert_eq!(outcome.receipt.confirmation(), Confirmation::Unverified);
}

#[cfg(unix)]
#[test]
fn late_entry_or_stage_replacement_holds_preparation_and_failure_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    for replace_entry in [true, false] {
        let (temp, activity) = fixture();
        let root_path = temp.path().to_owned();
        let (result, consumed) = run_with_media_hook(
            MediaPrimitive::CarrierConfirmation,
            move || {
                let path = if replace_entry {
                    root_path.join("backup-activity.lock")
                } else {
                    fs::read_dir(&root_path)?
                        .map(Result::unwrap)
                        .find(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with(".tmp-offload-")
                        })
                        .unwrap()
                        .path()
                };
                fs::rename(&path, path.with_extension("held"))?;
                if replace_entry {
                    fs::write(&path, b"keep")?;
                    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                } else {
                    fs::create_dir(path)
                }
            },
            || RootActivityCarrier::prepare(activity),
        );
        assert!(consumed);
        let failed = failure(result);
        assert_eq!(failed.receipt().root_durability(), Durability::Synced);
        assert_eq!(failed.receipt().confirmation(), Confirmation::Unverified);
        let path = failed.diagnostic_path().unwrap();
        let outcome = failed.retire_after_settlement();
        assert_eq!(
            outcome.receipt.effect(),
            ActivityCarrierRetirementEffect::NotAttempted
        );
        assert!(path.is_dir());
        if replace_entry {
            assert_eq!(
                fs::read(temp.path().join("backup-activity.lock")).unwrap(),
                b"keep"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn empty_replacement_before_final_check_is_preserved() {
    let (_temp, activity) = fixture();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    let path = carrier.diagnostic_path();
    let path_hook = path.clone();
    let original = path.with_extension("held");
    let original_hook = original.clone();
    let (outcome, consumed) = run_with_media_hook(
        MediaPrimitive::StageRemove,
        move || {
            fs::rename(&path_hook, original_hook)?;
            fs::create_dir(path_hook)
        },
        || carrier.retire_after_settlement(),
    );
    assert!(consumed);
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::Attempted
    );
    assert_eq!(outcome.receipt.confirmation(), Confirmation::Unverified);
    assert!(path.is_dir());
    assert!(original.is_dir());
}

#[cfg(unix)]
#[test]
fn uncooperating_empty_swap_at_native_rmdir_cut_is_explicitly_unsupported() {
    let (_temp, activity) = fixture();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    let path = carrier.diagnostic_path();
    let path_hook = path.clone();
    let original = path.with_extension("held");
    let original_hook = original.clone();
    let (outcome, consumed) = run_with_media_hook(
        MediaPrimitive::NativeDirectoryRemoveCut,
        move || {
            fs::rename(&path_hook, original_hook)?;
            fs::create_dir(path_hook)
        },
        || carrier.retire_after_settlement(),
    );
    assert!(consumed);
    // This deliberately violates the qualified-cooperation precondition. Native
    // rmdir removed the empty replacement; the original reservation survives.
    // The receipt cannot certify cooperation or unconditional exact retirement.
    assert_eq!(
        outcome.receipt.effect(),
        ActivityCarrierRetirementEffect::Removed
    );
    assert!(!path.exists());
    assert!(original.is_dir());
}

#[cfg(target_os = "linux")]
#[test]
fn ended_owners_empty_reservation_is_retired_by_the_next_lease_holder() {
    let (_temp, activity) = fixture();
    let root = activity.journal_root().clone();
    let carrier = RootActivityCarrier::prepare(activity).unwrap();
    let path = carrier.diagnostic_path();
    // While its owner lives, no other lease that could retire it exists.
    assert!(matches!(
        ActivityLease::try_acquire(root.clone()),
        Err(ActivityAdmissionError::Busy)
    ));
    // The owner ends without retiring it, as on a crash or restart.
    drop(carrier);
    assert!(path.is_dir());
    let next = ActivityLease::try_acquire(root.clone()).unwrap();
    assert_eq!(retire_orphan_root_carriers(&next).expect("retire"), 1);
    assert!(!path.exists());
    assert!(
        crate::activity_obligations::observe_obligations(&root)
            .unwrap()
            .entries()
            .is_empty()
    );
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn occupied_nested_and_intent_bound_reservations_stay_held() {
    let (temp, activity) = fixture();
    let root = activity.journal_root().clone();
    let occupied = temp.path().join(".tmp-offload-occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("payload"), b"unpublished copy").unwrap();
    let nested = temp.path().join("chronicle/20260101/.tmp-offload-nested");
    fs::create_dir_all(&nested).unwrap();
    let empty = temp.path().join(".tmp-offload-empty");
    fs::create_dir(&empty).unwrap();
    let intents = temp.path().join("health/offload/restore-intents");
    fs::create_dir_all(&intents).unwrap();
    let intent = intents.join(format!("{}.json", "a".repeat(32)));
    fs::write(&intent, b"{}").unwrap();
    // An interrupted restore may name any reservation, so all of them stay.
    assert_eq!(retire_orphan_root_carriers(&activity).expect("retire"), 0);
    assert!(empty.is_dir());
    fs::remove_file(&intent).unwrap();
    // Only the empty root reservation goes; content and nested stages stay.
    assert_eq!(retire_orphan_root_carriers(&activity).expect("retire"), 1);
    assert!(!empty.exists());
    assert_eq!(
        fs::read(occupied.join("payload")).unwrap(),
        b"unpublished copy"
    );
    assert!(nested.is_dir());
    assert_eq!(
        crate::activity_obligations::observe_obligations(&root)
            .unwrap()
            .entries()
            .len(),
        2
    );
}

/// A teardown that ended mid-run leaves its empty reservation; the next
/// teardown retires it under its own lease before observing anything.
#[cfg(any(target_os = "linux", windows))]
#[test]
fn ended_teardowns_empty_reservation_is_retired_by_the_next_teardown() {
    let (temp, teardown) = teardown_fixture();
    let root = teardown.journal_root().clone();
    let carrier = RootTeardownCarrier::prepare(teardown.clone()).unwrap();
    let path = carrier.diagnostic_path();
    drop(carrier);
    drop(teardown);
    assert!(path.is_dir());
    let occupied = temp.path().join(".tmp-offload-occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("payload"), b"unpublished copy").unwrap();
    let next = TeardownLease::try_acquire(root.clone()).unwrap();
    assert_eq!(
        retire_orphan_root_carriers_for_teardown(&next).expect("retire"),
        1
    );
    assert!(!path.exists());
    assert_eq!(
        fs::read(occupied.join("payload")).unwrap(),
        b"unpublished copy"
    );
}

#[cfg(all(unix, not(target_os = "ios")))]
#[test]
fn teardown_carrier_restart_process_helper() {
    let Some(journal) = std::env::var_os("SOLSTONE_TEARDOWN_CARRIER_TEST_JOURNAL") else {
        return;
    };
    let witness = std::env::var_os("SOLSTONE_TEARDOWN_CARRIER_TEST_WITNESS").unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(&journal)).unwrap());
    let lease = TeardownLease::try_acquire(root).unwrap();
    let carrier = RootTeardownCarrier::prepare(lease).unwrap();
    fs::write(
        witness,
        serde_json::to_vec(&carrier.recovery_observation().unwrap()).unwrap(),
    )
    .unwrap();
    // Test process termination ends every original native handle/lease. The
    // parent must open the exact original child under a new actual admission.
}

#[cfg(all(unix, not(target_os = "ios")))]
#[test]
fn teardown_carrier_restart_reacquires_original_and_refuses_replacements() {
    for mutation in ["none", "missing", "replacement", "nonempty"] {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("journal");
        fs::create_dir(&journal).unwrap();
        let witness = temp.path().join("observation.json");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "media::activity_carrier::tests::teardown_carrier_restart_process_helper",
                "--nocapture",
            ])
            .env("SOLSTONE_TEARDOWN_CARRIER_TEST_JOURNAL", &journal)
            .env("SOLSTONE_TEARDOWN_CARRIER_TEST_WITNESS", &witness)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let observed: StageObservation =
            serde_json::from_slice(&fs::read(witness).unwrap()).unwrap();
        let original = journal.join(&observed.directory_name);
        let moved = temp.path().join("original-carrier");
        match mutation {
            "missing" | "replacement" => {
                fs::rename(&original, &moved).unwrap();
                if mutation == "replacement" {
                    fs::create_dir(&original).unwrap();
                }
            }
            "nonempty" => fs::write(original.join("retained"), b"do not remove").unwrap(),
            _ => {}
        }
        let root = Arc::new(JournalRoot::open(&journal).unwrap());
        let lease = TeardownLease::try_acquire(root.clone()).unwrap();
        if mutation != "none" {
            assert!(
                RootTeardownCarrier::reacquire(lease.clone(), &observed).is_err(),
                "{mutation}"
            );
            if mutation == "replacement" {
                assert!(original.is_dir());
                fs::remove_dir(&original).unwrap();
            }
            if mutation == "missing" || mutation == "replacement" {
                assert!(moved.is_dir());
                fs::rename(&moved, &original).unwrap();
            }
            if mutation == "nonempty" {
                assert_eq!(
                    fs::read(original.join("retained")).unwrap(),
                    b"do not remove"
                );
                fs::remove_file(original.join("retained")).unwrap();
            }
        }
        let carrier = RootTeardownCarrier::reacquire(lease, &observed).unwrap();
        assert_eq!(carrier.recovery_observation().unwrap(), observed);
        assert_eq!(
            crate::activity_obligations::observe_with_carrier(&root, carrier.observation())
                .unwrap()
                .unresolved_entries()
                .count(),
            0
        );
        let retired = carrier.retire_after_settlement();
        assert_eq!(
            retired.receipt.effect(),
            ActivityCarrierRetirementEffect::Removed
        );
        assert_eq!(retired.receipt.confirmation(), Confirmation::Confirmed);
        assert_eq!(retired.receipt.root_durability(), Durability::Synced);
        assert!(retired.receipt.failures().is_empty());
        assert!(!original.exists());
    }
}
