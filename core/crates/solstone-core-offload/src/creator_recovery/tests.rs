// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_journal_io::media::{MediaPrimitive, run_with_media_hook};
use std::cell::Cell;
use std::fs;

struct Fixture {
    temp: tempfile::TempDir,
    root: Arc<JournalRoot>,
    selections: Vec<CreatorStageSelection>,
}

impl Fixture {
    fn new(count: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            b"{\"retention\":{\"raw_media\":\"keep\"}}",
        )
        .unwrap();
        let mut selections = Vec::new();
        for index in 0..count {
            let selection = CreatorStageSelection {
                stage: format!(".tmp-offload-{:032x}", index + 1),
                target: Target {
                    day: "20261009".into(),
                    stream: "import.audio".into(),
                    dir: format!("1200{index:02}_60"),
                },
                layout: SegmentLayout::Named,
                name: "imported_audio.wav".into(),
            };
            let original = temp
                .path()
                .join("chronicle")
                .join(&selection.target.day)
                .join(&selection.target.stream)
                .join(&selection.target.dir);
            fs::create_dir_all(&original).unwrap();
            fs::write(original.join(&selection.name), b"current").unwrap();
            let header = json!({"_solstone_processing":{"schema":"solstone.processing.v1","handler":"transcribe","state":"analyzed","reason_code":"ok","input_size":7,"attempted_at":"2026-10-09T00:00:00Z"}});
            fs::write(
                original.join("imported_audio.jsonl"),
                format!("{header}\n{{\"start\":0,\"text\":\"words\"}}\n"),
            )
            .unwrap();
            fs::write(original.join("custody.json"), b"retained custody").unwrap();
            fs::create_dir(temp.path().join(&selection.stage)).unwrap();
            fs::write(
                temp.path().join(&selection.stage).join(&selection.name),
                b"current",
            )
            .unwrap();
            selections.push(selection);
        }
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        // This existing sidecar belongs to the ended producer, not observation.
        drop(ActivityLease::try_acquire(Arc::clone(&root)).unwrap());
        Self {
            temp,
            root,
            selections,
        }
    }
    fn stage(&self, index: usize) -> PathBuf {
        self.temp.path().join(&self.selections[index].stage)
    }
    fn original_dir(&self, index: usize) -> PathBuf {
        let target = &self.selections[index].target;
        self.temp
            .path()
            .join("chronicle")
            .join(&target.day)
            .join(&target.stream)
            .join(&target.dir)
    }
    fn enter(&self) -> CreatorStageRecovery {
        CreatorStageRecovery::enter(
            Arc::clone(&self.root),
            &self.selections,
            DateTime::parse_from_rfc3339("2026-10-10T00:00:00Z").unwrap(),
            &|| Ok(()),
        )
    }
    fn domain_bytes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(root: &Path, directory: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() == "backup-activity.lock"
                    || entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".tmp-offload-")
                {
                    continue;
                }
                if entry.file_type().unwrap().is_dir() {
                    walk(root, &entry.path(), out);
                } else if entry.file_type().unwrap().is_file() {
                    out.push((
                        entry.path().strip_prefix(root).unwrap().to_owned(),
                        fs::read(entry.path()).unwrap(),
                    ));
                }
            }
        }
        let mut bytes = Vec::new();
        walk(self.temp.path(), self.temp.path(), &mut bytes);
        bytes.sort();
        bytes
    }
}

fn observe(f: &Fixture) -> CreatorStageRecovery {
    CreatorStageRecovery::observe(
        Arc::clone(&f.root),
        DateTime::parse_from_rfc3339("2026-10-10T00:00:00Z").unwrap(),
        &|| Ok(()),
    )
}

#[test]
fn clean_observation_creates_no_lock_or_apply_footprint() {
    let temp = tempfile::tempdir().unwrap();
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let owner = CreatorStageRecovery::observe(
        root,
        DateTime::parse_from_rfc3339("2026-10-10T00:00:00Z").unwrap(),
        &|| Ok(()),
    );
    assert!(owner.refusal.is_none(), "{}", owner.report());
    assert!(owner.confirmation(&|| Ok(())).unwrap().stages.is_empty());
    assert!(!owner.report()["attempted"].as_bool().unwrap());
    drop(owner);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn discovery_requires_unique_current_processed_original_and_existing_exclusion() {
    for case in 0..5 {
        let f = Fixture::new(if case == 1 { 2 } else { 1 });
        match case {
            0 => {}
            1 => {} // Both live groups have identical bytes: ambiguous.
            2 => fs::remove_file(f.original_dir(0).join("imported_audio.wav")).unwrap(),
            3 => fs::write(f.original_dir(0).join("screen.png"), b"unprocessed").unwrap(),
            _ => fs::remove_file(f.temp.path().join("backup-activity.lock")).unwrap(),
        }
        let before = f.domain_bytes();
        let owner = observe(&f);
        assert_eq!(
            owner.refusal.is_none(),
            case == 0,
            "case={case} {}",
            owner.report()
        );
        if case == 0 {
            let confirmation = owner.confirmation(&|| Ok(())).unwrap();
            assert_eq!(confirmation.stages.len(), 1);
            assert_eq!(
                confirmation.stages[0].original.as_ref().unwrap().selection,
                f.selections[0]
            );
        }
        assert_eq!(f.domain_bytes(), before);
        assert!(f.stage(0).join("imported_audio.wav").exists());
        if case == 4 {
            assert!(!f.temp.path().join("backup-activity.lock").exists());
        }
    }
}

#[test]
fn complete_confirmation_refuses_valid_replacements_population_and_config_drift() {
    for case in 0..5 {
        let f = Fixture::new(1);
        let empty = f
            .temp
            .path()
            .join(".tmp-offload-ffffffffffffffffffffffffffffffff");
        if case == 3 {
            fs::create_dir(&empty).unwrap();
        }
        let owner = observe(&f);
        let confirmation = owner.confirmation(&|| Ok(())).unwrap();
        drop(owner);
        match case {
            0 | 1 => {
                let dir = if case == 0 {
                    f.stage(0)
                } else {
                    f.original_dir(0)
                };
                fs::rename(dir.join("imported_audio.wav"), dir.join("old.wav")).unwrap();
                fs::write(dir.join("imported_audio.wav"), b"current").unwrap();
                fs::remove_file(dir.join("old.wav")).unwrap();
            }
            2 => fs::create_dir(&empty).unwrap(),
            3 => fs::remove_dir(&empty).unwrap(),
            _ => fs::write(
                f.temp.path().join("config/journal.json"),
                b"{\"retention\":{\"raw_media\":\"keep\"},\"timezone\":\"UTC\"}",
            )
            .unwrap(),
        }
        let mut fresh = observe(&f);
        assert!(fresh.refusal.is_none(), "case={case} {}", fresh.report());
        fresh.apply_confirmed(&confirmation, &|| Ok(()));
        assert_eq!(fresh.report()["attempted"], false, "case={case}");
        assert_eq!(
            fresh.report()["refusal"],
            "creator_recovery_confirmation_changed"
        );
        assert!(f.stage(0).join("imported_audio.wav").exists());
    }
}

#[test]
fn explicit_readonly_reconciliation_preserves_old_unknown_and_requires_originals_and_clear_census()
{
    for case in 0..3 {
        let f = Fixture::new(1);
        let mut owner = observe(&f);
        let prior = owner.confirmation(&|| Ok(())).unwrap();
        owner.apply_confirmed(&prior, &|| Ok(()));
        assert_eq!(owner.report()["local_complete"], true, "{}", owner.report());
        drop(owner);
        if case == 1 {
            fs::write(f.original_dir(0).join("imported_audio.wav"), b"changed").unwrap();
        }
        if case == 2 {
            fs::create_dir(
                f.temp
                    .path()
                    .join(".tmp-offload-ffffffffffffffffffffffffffffffff"),
            )
            .unwrap();
        }
        let before = f.domain_bytes();
        let result = CreatorStageRecovery::reconcile(
            Arc::clone(&f.root),
            &prior,
            DateTime::parse_from_rfc3339("2026-10-10T00:00:00Z").unwrap(),
            &|| Ok(()),
        );
        assert_eq!(
            result.report()["current_local_state_reconciled"],
            case == 0,
            "case={case} {}",
            result.report()
        );
        assert_eq!(result.report()["previous_outcome"], "unknown_or_partial");
        assert_eq!(result.report()["local_complete"], false);
        assert_eq!(result.report()["attempted"], false);
        assert_eq!(result.report()["remote_outcome"], "unknown");
        assert_eq!(f.domain_bytes(), before);
        assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
    }
}

#[test]
fn mobile_review_row_limit_refuses_whole_population() {
    let f = Fixture::new(0);
    for index in 0..65 {
        fs::create_dir(f.temp.path().join(format!(".tmp-offload-{index:032x}"))).unwrap();
    }
    let mut owner = observe(&f);
    owner.apply(&|| Ok(()));
    assert!(owner.refusal.is_some());
    assert_eq!(
        fs::read_dir(f.temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp-offload-"))
            .count(),
        65
    );
}

#[test]
fn explicit_local_settlement_preserves_keep_forever_originals_and_remote_unknown() {
    let f = Fixture::new(2);
    let empty = f
        .temp
        .path()
        .join(".tmp-offload-ffffffffffffffffffffffffffffffff");
    fs::create_dir(&empty).unwrap();
    let before = f.domain_bytes();
    let mut owner = f.enter();
    assert!(owner.refusal.is_none(), "{}", owner.report());
    let policy = solstone_core_retention::policy::policy_from_journal_config(
        owner
            .context
            .as_ref()
            .unwrap()
            .config
            .0
            .config
            .as_ref()
            .unwrap(),
    );
    assert_eq!(
        policy.rule_for("import.audio"),
        solstone_core_retention::policy::Rule::keep()
    );
    assert!(!owner.report()["attempted"].as_bool().unwrap());
    assert!(f.stage(0).exists() && f.stage(1).exists() && empty.exists());
    assert!(!f.temp.path().join("config/journal.json.lock").exists());
    owner.apply(&|| Ok(()));
    assert_eq!(owner.report()["local_complete"], true, "{}", owner.report());
    assert!(!f.stage(0).exists() && !f.stage(1).exists() && !empty.exists());
    assert_eq!(f.domain_bytes(), before);
    assert_eq!(owner.report()["remote_outcome"], "unknown");
    assert_eq!(owner.report()["replay_authorized"], false);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
    drop(owner);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_ok());
}

#[test]
fn invalid_later_member_refuses_whole_population_before_first_removal() {
    let f = Fixture::new(2);
    fs::write(f.stage(1).join("extra"), b"keep").unwrap();
    let before = f.domain_bytes();
    let mut owner = f.enter();
    assert!(owner.refusal.is_some());
    owner.apply(&|| Ok(()));
    assert!(f.stage(0).join("imported_audio.wav").exists());
    assert!(f.stage(1).join("extra").exists());
    assert_eq!(f.domain_bytes(), before);
}

#[test]
fn restore_intent_and_busy_creator_are_preserved_refusals() {
    for intent in [false, true] {
        let f = Fixture::new(1);
        let lease = if intent {
            let p = f.temp.path().join("health/offload/restore-intents");
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("unknown"), b"owed facts").unwrap();
            None
        } else {
            Some(ActivityLease::try_acquire(Arc::clone(&f.root)).unwrap())
        };
        let before = f.domain_bytes();
        let mut owner = f.enter();
        assert!(owner.refusal.is_some());
        owner.apply(&|| Ok(()));
        assert!(f.stage(0).join("imported_audio.wav").exists());
        assert_eq!(f.domain_bytes(), before);
        drop(lease);
    }
}

#[test]
fn changed_original_or_stage_after_observation_refuses_before_first_effect() {
    for staged in [false, true] {
        let f = Fixture::new(1);
        let mut owner = f.enter();
        assert!(owner.refusal.is_none());
        let path = if staged {
            f.stage(0)
        } else {
            f.original_dir(0)
        }
        .join("imported_audio.wav");
        let old = fs::metadata(&path).unwrap();
        fs::write(&path, b"changed").unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(old.ino(), fs::metadata(&path).unwrap().ino());
        owner.apply(&|| Ok(()));
        assert!(owner.refusal.is_some());
        assert_eq!(
            owner.report()["stages"][0]["facts"]["leaf_effect"],
            "NotAttempted"
        );
        assert!(f.stage(0).exists());
    }
}

#[test]
fn later_original_stage_or_group_drift_preserves_first_effect_and_stops_next() {
    for change in 0..3 {
        let f = Fixture::new(2);
        let mut owner = f.enter();
        assert!(owner.refusal.is_none());
        let changed = Cell::new(false);
        owner.apply(&|| {
            if !f.stage(0).exists() && !changed.replace(true) {
                match change {
                    0 => fs::write(f.original_dir(1).join("imported_audio.wav"), b"changed")?,
                    1 => fs::write(f.stage(1).join("imported_audio.wav"), b"changed")?,
                    _ => fs::write(f.original_dir(1).join("screen.png"), b"unprocessed")?,
                }
            }
            Ok(())
        });
        assert!(changed.get());
        assert!(owner.refusal.is_some());
        assert!(!f.stage(0).exists());
        assert!(f.stage(1).exists());
        assert_eq!(
            owner.report()["stages"][0]["complete"],
            true,
            "{}",
            owner.report()
        );
        assert_eq!(
            owner.report()["stages"][1]["facts"]["leaf_effect"],
            "NotAttempted"
        );
    }
}

#[test]
fn parent_sync_failure_keeps_native_effects_and_actual_exclusion() {
    let f = Fixture::new(2);
    let mut owner = f.enter();
    assert!(owner.refusal.is_none());
    let (_, used) = run_with_media_hook(
        MediaPrimitive::ParentSync,
        || Err(io::Error::other("held root sync")),
        || owner.apply(&|| Ok(())),
    );
    assert!(used);
    let report = owner.report();
    assert_eq!(report["local_complete"], false);
    assert_eq!(report["stages"][0]["facts"]["leaf_effect"], "Requested");
    assert_eq!(
        report["stages"][0]["facts"]["directory_absence"],
        "Confirmed"
    );
    assert_eq!(
        report["stages"][0]["facts"]["parent_durability"],
        "Unproven"
    );
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
    assert!(f.stage(1).exists());
    owner.apply(&|| Ok(()));
    assert!(f.stage(1).exists());
    drop(owner);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_ok());
}

#[test]
fn unwind_after_leaf_effect_retains_partial_facts_without_retry() {
    let f = Fixture::new(1);
    let mut owner = f.enter();
    assert!(owner.refusal.is_none());
    let (_, used) = run_with_media_hook(
        MediaPrimitive::NativeDirectoryRemoveCut,
        || panic!("actual native cut"),
        || owner.apply(&|| Ok(())),
    );
    assert!(used);
    let report = owner.report();
    assert_eq!(report["local_complete"], false);
    assert_eq!(report["refusal"], "creator_recovery_interrupted");
    assert_eq!(report["stages"][0]["facts"]["leaf_effect"], "Requested");
    assert_eq!(
        report["stages"][0]["facts"]["directory_effect"],
        "Uncertain"
    );
    assert!(f.stage(0).exists());
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
    drop(owner);
    assert!(f.stage(0).exists());
}

#[test]
fn replaced_root_never_reopens_replacement_for_settlement() {
    let f = Fixture::new(1);
    let mut owner = f.enter();
    assert!(owner.refusal.is_none());
    let original = f.temp.path().with_extension("creator-original");
    fs::rename(f.temp.path(), &original).unwrap();
    fs::create_dir(f.temp.path()).unwrap();
    fs::write(f.temp.path().join("sentinel"), b"protected").unwrap();
    owner.apply(&|| Ok(()));
    assert!(owner.refusal.is_some());
    assert_eq!(fs::read_dir(f.temp.path()).unwrap().count(), 1);
    assert!(
        original
            .join(&f.selections[0].stage)
            .join("imported_audio.wav")
            .exists()
    );
    drop(owner);
    fs::remove_dir_all(original).unwrap();
}
