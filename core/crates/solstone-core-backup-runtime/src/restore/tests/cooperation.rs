// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_journal_io::{JournalRoot, activity::ActivityLease};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

fn assert_busy(path: &Path) {
    let root = Arc::new(JournalRoot::open(path).unwrap());
    assert!(matches!(
        ActivityLease::try_acquire(root),
        Err(solstone_core_journal_io::activity::ActivityAdmissionError::Busy)
    ));
}

fn successful_script() -> Script {
    Script::new(vec![
        output(0, CAPTURED_SNAPSHOTS),
        output(0, CAPTURED_RESTORE),
        output(0, ""),
    ])
}

#[test]
fn busy_or_unresolved_population_refuses_before_tools_and_recording() {
    for variant in 0..5 {
        let journal = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let held = if variant == 0 {
            Some(ActivityLease::try_acquire(root).unwrap())
        } else {
            if variant == 1 {
                fs::create_dir(journal.path().join(".tmp-offload-current")).unwrap();
            } else if variant == 2 {
                let intents = journal.path().join("health/offload/restore-intents");
                fs::create_dir_all(&intents).unwrap();
                fs::write(intents.join("unknown.json"), b"{}").unwrap();
            } else if variant == 3 {
                let name = std::ffi::OsStr::from_bytes(b".tmp-offload-\xff");
                fs::create_dir(journal.path().join(name)).unwrap();
            } else {
                std::os::unix::fs::symlink(journal.path(), journal.path().join("health")).unwrap();
            }
            None
        };
        let runner = Script::new(vec![]);
        let recorder = Recorder::new();
        let result = restore_journal(
            journal.path(),
            &services(&runner, &TestClock, &Maintenance),
            &recorder,
            destination(),
            "not a recovery key",
        );
        assert_eq!(
            result.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert!(runner.calls.borrow().is_empty());
        assert!(recorder.calls.borrow().is_empty());
        drop(held);
    }
}

#[test]
fn missing_destination_preserves_unix_names_and_creates_only_its_own_lock() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join(".Mixed Parent/Journal café");
    let recorder = Recorder::new();
    let runner = Script::new(vec![]);
    let result = restore_journal(
        &target,
        &services(&runner, &TestClock, &Maintenance),
        &recorder,
        destination(),
        "invalid key",
    );
    assert_eq!(
        result.reason_code.as_deref(),
        Some(RESTORE_REASON_INVALID_KEY)
    );
    assert!(target.is_dir());
    assert!(target.join("backup-activity.lock").is_file());
    assert!(!temp.path().join("backup-activity.lock").exists());
    assert!(
        !target
            .parent()
            .unwrap()
            .join("backup-activity.lock")
            .exists()
    );
    assert_eq!(recorder.calls.borrow().len(), 1);
    assert!(runner.calls.borrow().is_empty());
}

#[test]
fn changed_root_or_named_lock_after_listing_refuses_payload_and_local_writes() {
    use std::os::unix::fs::PermissionsExt;
    struct AfterList<'a> {
        script: Script,
        journal: &'a Path,
        moved: &'a Path,
        replace_root: bool,
    }
    impl ToolRunner for AfterList<'_> {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            assert_eq!(request.argv[0], "snapshots");
            assert_busy(self.journal);
            let output = self.script.run(request)?;
            if self.replace_root {
                fs::rename(self.journal, self.moved).unwrap();
                fs::create_dir(self.journal).unwrap();
                fs::write(self.journal.join("replacement"), b"keep").unwrap();
            } else {
                let lock = self.journal.join("backup-activity.lock");
                fs::rename(&lock, self.journal.join("original.lock")).unwrap();
                fs::write(&lock, b"").unwrap();
                fs::set_permissions(lock, fs::Permissions::from_mode(0o600)).unwrap();
            }
            Ok(output)
        }
    }
    for replace_root in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("journal");
        let moved = temp.path().join("original");
        fs::create_dir(&journal).unwrap();
        let keys = solstone_core_backup::generate_and_store_keys(&journal).unwrap();
        let config_before = solstone_core_backup::get_backup_config(&journal).unwrap();
        let runner = AfterList {
            script: Script::new(vec![output(0, CAPTURED_SNAPSHOTS)]),
            journal: &journal,
            moved: &moved,
            replace_root,
        };
        let recorder = Recorder::new();
        let result = restore_journal(
            &journal,
            &services(&runner, &TestClock, &Maintenance),
            &recorder,
            destination(),
            &keys.recovery_key,
        );
        assert_eq!(
            result.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(runner.script.calls.borrow().len(), 1);
        assert!(recorder.calls.borrow().is_empty());
        let original = if replace_root { &moved } else { &journal };
        assert_eq!(
            solstone_core_backup::get_backup_config(original).unwrap(),
            config_before
        );
        if replace_root {
            assert_eq!(fs::read(journal.join("replacement")).unwrap(), b"keep");
        }
    }
}

struct PathRecorder {
    identity: (u64, u64),
    facts: Recorder,
}

impl RestoreRecorder for PathRecorder {
    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        journal: &Path,
        status: &str,
        time: Value,
        reason: Value,
        scope: &str,
        day: Value,
        selected: Value,
        restored: Value,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) -> Result<(), solstone_core_backup::BackupError> {
        let meta = fs::metadata(journal).unwrap();
        assert_eq!((meta.dev(), meta.ino()), self.identity);
        assert_busy(journal);
        fs::write(journal.join("recorded-at-original-root"), b"recorded").unwrap();
        self.facts.record(
            journal,
            status,
            time,
            reason,
            scope,
            day,
            selected,
            restored,
            files_expected,
            files_restored,
            bytes_expected,
            bytes_restored,
        )
    }
}

#[test]
fn callback_root_replacement_keeps_original_effects_and_exclusion_on_return_or_unwind() {
    struct Replace<'a> {
        original: &'a Path,
        moved: &'a Path,
        mode: u8,
    }
    impl JournalMaintenance for Replace<'_> {
        fn rebuild_body_history(&self, journal: &Path) -> Result<(), JournalMaintenanceError> {
            assert_busy(self.original);
            fs::rename(self.original, self.moved).unwrap();
            fs::create_dir(self.original).unwrap();
            fs::write(self.original.join("replacement"), b"keep").unwrap();
            fs::write(journal.join("callback-at-original-root"), b"effect").unwrap();
            assert_busy(self.moved);
            match self.mode {
                0 => Ok(()),
                1 => Err(JournalMaintenanceError),
                _ => panic!("callback unwind with original Activity still held"),
            }
        }
        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("later callback must not run after binding loss")
        }
    }
    for mode in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("journal");
        let moved = temp.path().join("original");
        fs::create_dir(&journal).unwrap();
        let keys = solstone_core_backup::generate_and_store_keys(&journal).unwrap();
        let config_before = solstone_core_backup::get_backup_config(&journal).unwrap();
        let meta = fs::metadata(&journal).unwrap();
        let recorder = PathRecorder {
            identity: (meta.dev(), meta.ino()),
            facts: Recorder::new(),
        };
        let runner = successful_script();
        let maintenance = Replace {
            original: &journal,
            moved: &moved,
            mode,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_journal(
                &journal,
                &services(&runner, &TestClock, &maintenance),
                &recorder,
                destination(),
                &keys.recovery_key,
            )
        }));
        assert!(moved.join("callback-at-original-root").is_file());
        assert!(!journal.join("callback-at-original-root").exists());
        assert_eq!(fs::read(journal.join("replacement")).unwrap(), b"keep");
        assert_eq!(
            solstone_core_backup::get_backup_config(&moved).unwrap(),
            config_before
        );
        if mode == 2 {
            assert!(result.is_err());
            assert!(recorder.facts.calls.borrow().is_empty());
        } else {
            let result = result.unwrap();
            assert_eq!(result.status, "error");
            assert_eq!(
                result.reason_code.as_deref(),
                Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
            );
            assert_eq!(result.bytes_restored, Some(11));
            assert!(moved.join("recorded-at-original-root").is_file());
            assert!(!journal.join("recorded-at-original-root").exists());
            assert_eq!(recorder.facts.calls.borrow().len(), 1);
        }
        ActivityLease::try_acquire(Arc::new(JournalRoot::open(&moved).unwrap())).unwrap();
    }
}

#[test]
fn late_obligation_preserves_payload_facts_and_stops_active_binding_restore() {
    struct NeverMaintenance;
    impl JournalMaintenance for NeverMaintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("rebuild after unresolved restore obligation");
        }
        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            panic!("scan after unresolved restore obligation");
        }
    }
    struct Late<'a> {
        script: Script,
        journal: &'a Path,
    }
    impl ToolRunner for Late<'_> {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            assert_busy(self.journal);
            if request.argv.first().is_some_and(|arg| arg == "restore") && !is_dry_run(request) {
                assert!(
                    request.argv.windows(2).any(|args| {
                        args[0] == "--exclude" && args[1] == "backup-activity.lock"
                    })
                );
                let named = fs::metadata(self.journal.join("backup-activity.lock")).unwrap();
                assert!(request.pass_fds.iter().any(|fd| {
                    use std::os::fd::AsRawFd;
                    let meta = fs::metadata(format!("/proc/self/fd/{}", fd.as_raw_fd())).unwrap();
                    (meta.dev(), meta.ino()) == (named.dev(), named.ino())
                }));
                let mut retired = solstone_core_backup::load_hosted_binding(self.journal).unwrap();
                retired.broker_token = "snapshot-token".into();
                solstone_core_backup::save_hosted_binding(self.journal, &retired).unwrap();
                fs::create_dir(self.journal.join(".tmp-offload-late")).unwrap();
            }
            self.script.run(request)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let keys = solstone_core_backup::generate_and_store_keys(temp.path()).unwrap();
    let active = solstone_core_backup::HostedBinding {
        broker_endpoint: "https://services.example".into(),
        account_id: "owner".into(),
        instance_id: "instance".into(),
        bucket: "bucket".into(),
        prefix: "owner/instance/".into(),
        broker_token: "original-token".into(),
    };
    solstone_core_backup::save_hosted_binding(temp.path(), &active).unwrap();
    let config_before = solstone_core_backup::get_backup_config(temp.path()).unwrap();
    let runner = Late {
        script: successful_script(),
        journal: temp.path(),
    };
    let recorder = Recorder::new();
    let result = restore_journal(
        temp.path(),
        &services(&runner, &TestClock, &NeverMaintenance),
        &recorder,
        destination(),
        &keys.recovery_key,
    );
    assert_eq!(result.status, "error");
    assert_eq!(
        result.reason_code.as_deref(),
        Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
    );
    assert_eq!(result.bytes_restored, Some(11));
    assert!(!result.resumable);
    assert_eq!(runner.script.calls.borrow().len(), 2);
    assert_eq!(recorder.calls.borrow().len(), 1);
    assert_eq!(recorder.calls.borrow()[0].bytes, json!([11, 11]));
    assert_eq!(
        solstone_core_backup::get_backup_config(temp.path()).unwrap(),
        config_before
    );
    assert_eq!(
        solstone_core_backup::load_hosted_binding(temp.path())
            .unwrap()
            .broker_token,
        "snapshot-token"
    );
    assert!(temp.path().join(".tmp-offload-late").is_dir());
}
