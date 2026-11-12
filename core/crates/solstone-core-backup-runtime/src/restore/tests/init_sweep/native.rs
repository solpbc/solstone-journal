// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A restore during setup against real restic. Needs `SOLSTONE_NATIVE_RESTIC`.

use super::*;
use crate::runner::run_restic;
use solstone_core_journal_io::operational_log::{
    OplogFormat, OplogWriter, catalog_oplogs, create_oplog_at,
};
use solstone_core_journal_io::{FileLease, LeaseOptions, acquire_file_lease};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq)]
enum Stop {
    Never,
    BeforeRestore,
    AfterRestore,
    RestoreFails,
    CheckFails,
    ReportAfterRestore,
}

struct Restic {
    stop: Cell<Stop>,
    restores: Cell<usize>,
    exclusions: RefCell<Vec<PathBuf>>,
    make_late_log: Cell<bool>,
    late_log: RefCell<Option<OplogWriter>>,
}

impl Restic {
    fn stopping(stop: Stop) -> Self {
        Self {
            stop: Cell::new(stop),
            restores: Cell::new(0),
            exclusions: RefCell::new(Vec::new()),
            make_late_log: Cell::new(false),
            late_log: RefCell::new(None),
        }
    }

    fn execute(request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        crate::runner::SystemToolRunner.run(request)
    }
}

impl ToolRunner for Restic {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let flag = |name: &str| request.argv.iter().any(|arg| arg == name);
        let restore = request.argv.first().is_some_and(|arg| arg == "restore");
        let check = request.argv.first().is_some_and(|arg| arg == "check");
        let real_restore = restore && !flag("--dry-run");
        if let Some(index) = request.argv.iter().position(|arg| arg == "--exclude-file") {
            let path = PathBuf::from(&request.argv[index + 1]);
            fs::read(&path).expect("restore exclusion file is readable during its child");
            self.exclusions.borrow_mut().push(path);
        }
        if real_restore && self.make_late_log.replace(false) {
            let at = request
                .argv
                .iter()
                .position(|arg| arg == "--target")
                .unwrap();
            let target = PathBuf::from(&request.argv[at + 1]);
            let mut writer = create_oplog_at(
                solstone_core_journal_io::JournalRoot::open(&target).unwrap(),
                "late",
                "worker",
                OplogFormat::Log,
                chrono::DateTime::parse_from_rfc3339("2026-11-12T12:00:00+00:00").unwrap(),
            )
            .unwrap();
            writer
                .append_bytes_checked(b"late before restore\n", &|| Ok(()))
                .unwrap();
            self.late_log.replace(Some(writer));
        }

        if real_restore {
            self.restores.set(self.restores.get() + 1);
        }
        match self.stop.get() {
            Stop::BeforeRestore if real_restore => {
                self.stop.set(Stop::Never);
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            Stop::RestoreFails if real_restore => return Ok(output(1, "")),
            Stop::CheckFails if check => return Ok(output(1, "")),
            Stop::ReportAfterRestore if restore && flag("--delete") => {
                self.stop.set(Stop::Never);
                return Ok(output(1, ""));
            }
            Stop::AfterRestore if real_restore => {
                Self::execute(request)?;
                self.stop.set(Stop::Never);
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            _ => {}
        }
        Self::execute(request)
    }

    fn run_cancellable(
        &self,
        request: &ToolRequest<'_>,
        _: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> io::Result<ToolOutput> {
        self.run(request)
    }
}

const AUTHORITY: &[&str] = &[
    "health/parent-loss/active-generation.json",
    "health/parent-loss/generations/3/admissions/sense-target/acknowledgement.json",
    "health/speakers-analyze/install-generation.json",
    "health/speakers-analyze/owner.json",
];
const LEASES: &[&str] = &[
    "health/parent-loss/coordinator.lease",
    "health/speakers-analyze/install-generation.lock",
];
const OWNER_CONTROLS: &[&str] = &[
    "health/retention.log",
    "health/parent-loss-notes/active-generation.json",
    "chronicle/20261112/health/120000_60/oplog--raw.log",
    "chronicle/20261112/health/120000_60/health/parent-loss/active-generation.json",
];

struct Fixture {
    journal: tempfile::TempDir,
    _repository: tempfile::TempDir,
    destination: Destination,
    key: String,
    binary: PathBuf,
    logs: Vec<RefCell<OplogWriter>>,
    _leases: Vec<FileLease>,
    extra_controls: &'static [&'static str],
}

impl Fixture {
    /// A journal, backed up, then lived in: changed, added to and removed from.
    fn lived_in() -> Self {
        Self::lived_in_with_controls(&[])
    }

    fn lived_in_with_controls(extra_controls: &'static [&'static str]) -> Self {
        let binary = PathBuf::from(
            std::env::var_os("SOLSTONE_NATIVE_RESTIC").expect("native restic required"),
        );
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let stored = solstone_core_backup::generate_and_store_keys(root).unwrap();
        let key = parse_recovery_key(&stored.recovery_key).unwrap();
        restore_sweep::mark_init_restore(root).unwrap();
        let repository = tempfile::tempdir().unwrap();
        let mut destination = destination();
        destination.repository = repository.path().join("r").to_string_lossy().into_owned();
        let restic = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            let output = run_restic(
                &crate::runner::SystemToolRunner,
                &args,
                &destination.repository,
                &key,
                binary.as_path(),
                None,
                false,
                None,
                None,
                &[],
            )
            .unwrap();
            assert_eq!(output.returncode, 0, "{args:?}");
        };
        restic(&["init"]);

        write(root, "notes/same.md", "same");
        write(root, "notes/changed.md", "as backed up");
        write(root, "notes/removed-since.md", "still here then");
        write(root, "config/removals/old.json", "old entry");
        write(
            root,
            "chronicle/20260101/s/090000_300/audio.flac",
            "the recording",
        );
        write(root, "entities/cache/entity.json", "owner cache");
        write(root, "cache/model", "snapshot's model");
        write(root, ".trash/kept/item.json", "trashed before the backup");
        for path in AUTHORITY.iter().chain(LEASES) {
            write(root, path, "foreign source authority");
        }
        for path in OWNER_CONTROLS.iter().chain(extra_controls) {
            write(root, path, "owner original");
        }
        let instant = chrono::DateTime::parse_from_rfc3339("2026-11-12T12:00:00+00:00").unwrap();
        let opened = solstone_core_journal_io::JournalRoot::open(root).unwrap();
        let mut older_log = create_oplog_at(
            solstone_core_journal_io::JournalRoot::open(root).unwrap(),
            "service",
            "supervisor",
            OplogFormat::Log,
            instant,
        )
        .unwrap();
        older_log
            .append_bytes_checked(b"before snapshot\n", &|| Ok(()))
            .unwrap();
        restic(&["backup", &root.display().to_string()]);
        older_log
            .append_bytes_checked(b"destination before restore\n", &|| Ok(()))
            .unwrap();
        let mut newer_log =
            create_oplog_at(opened, "sense", "sense", OplogFormat::Log, instant).unwrap();
        newer_log
            .append_bytes_checked(b"destination before restore\n", &|| Ok(()))
            .unwrap();
        for path in AUTHORITY.iter().chain(LEASES) {
            write(root, path, "destination authority");
        }
        let leases = LEASES
            .iter()
            .map(|path| {
                acquire_file_lease(root.join(path), LeaseOptions::default())
                    .unwrap()
                    .unwrap()
            })
            .collect();
        for path in OWNER_CONTROLS.iter().chain(extra_controls) {
            write(root, path, "edited owner original");
        }

        write(root, "notes/changed.md", "edited since");
        write(root, "notes/newer.md", "written since");
        write(root, "notes/deep/er/newest.md", "written since, deeper");
        fs::remove_file(root.join("notes/removed-since.md")).unwrap();
        write(root, "config/removals/newer.json", "newer entry");
        fs::remove_file(root.join("chronicle/20260101/s/090000_300/audio.flac")).unwrap();
        write(root, "chronicle/20260101/s/090000_300/tombstone.json", "{}");
        write(root, "entities/cache/entity.json", "changed owner cache");
        write(root, "cache/model", "this computer's model");
        write(root, "health/service.log", "this computer's log");
        write(root, ".trash/since/item.json", "trashed since the backup");
        Self {
            journal,
            _repository: repository,
            destination,
            key,
            binary,
            logs: vec![RefCell::new(older_log), RefCell::new(newer_log)],
            _leases: leases,
            extra_controls,
        }
    }

    fn restic(&self, args: &[&str]) -> crate::runner::ResticResult {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let output = run_restic(
            &crate::runner::SystemToolRunner,
            &args,
            &self.destination.repository,
            &self.key,
            self.binary.as_path(),
            None,
            args[0] == "snapshots",
            None,
            None,
            &[],
        )
        .unwrap();
        assert_eq!(output.returncode, 0, "{args:?}");
        output
    }

    /// Every snapshot as (time, tags), oldest first.
    fn snapshots(&self) -> Vec<(String, Vec<String>)> {
        let listed = self.restic(&["snapshots"]).json.unwrap();
        let mut found: Vec<(String, Vec<String>)> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                let tags = entry["tags"].as_array().map_or_else(Vec::new, |tags| {
                    tags.iter()
                        .map(|tag| tag.as_str().unwrap().to_owned())
                        .collect()
                });
                (entry["time"].as_str().unwrap().to_owned(), tags)
            })
            .collect();
        found.sort();
        found
    }

    fn snapshot_id(&self) -> String {
        let output = run_restic(
            &crate::runner::SystemToolRunner,
            &["snapshots".to_owned()],
            &self.destination.repository,
            &self.key,
            self.binary.as_path(),
            None,
            true,
            None,
            None,
            &[],
        )
        .unwrap();
        output.json.unwrap()[0]["id"].as_str().unwrap().to_owned()
    }

    fn restore_mobile(&self, runner: &Restic, drained: &Cell<usize>) -> RestoreOutcome {
        let clock = TestClock;
        let maintenance = Maintenance;
        let services = BackupServices {
            restic: Some(self.binary.as_path().into()),
            ..services(runner, &clock, &maintenance)
        };
        let mut drain = || {
            drained.set(drained.get() + 1);
            Ok(())
        };
        restore_mobile_journal(
            Arc::new(solstone_core_journal_io::JournalRoot::open(self.journal.path()).unwrap()),
            &services,
            &Recorder::new(),
            self.destination.clone(),
            &self.key,
            MobileRestoreIdentity::Byo,
            MobileSelection {
                snapshot_id: self.snapshot_id(),
                subtree: self.journal.path().display().to_string(),
            },
            &mut drain,
        )
    }

    fn restore(&self, runner: &Restic) -> RestoreOutcome {
        let drained = Cell::new(0);
        self.restore_mobile(runner, &drained)
    }

    fn assert_is_the_snapshot(&self) {
        let root = self.journal.path();
        for path in self.extra_controls {
            assert_eq!(read(root, path), "owner original", "{path}");
        }
        assert_eq!(read(root, "notes/same.md"), "same");
        assert_eq!(read(root, "notes/changed.md"), "as backed up");
        assert_eq!(read(root, "notes/removed-since.md"), "still here then");
        assert!(!root.join("notes/newer.md").exists());
        assert!(!root.join("notes/deep").exists());
        // Removal history is the snapshot's.
        assert_eq!(read(root, "config/removals/old.json"), "old entry");
        assert!(!root.join("config/removals/newer.json").exists());
        // A tombstone over a recording the snapshot holds leaves with the rest.
        assert_eq!(
            read(root, "chronicle/20260101/s/090000_300/audio.flac"),
            "the recording"
        );
        assert!(
            !root
                .join("chronicle/20260101/s/090000_300/tombstone.json")
                .exists()
        );
        // Only the top-level cache is this computer's.
        assert_eq!(read(root, "entities/cache/entity.json"), "owner cache");
        assert_eq!(read(root, "cache/model"), "this computer's model");
        assert_eq!(read(root, "health/service.log"), "this computer's log");
        for path in OWNER_CONTROLS.iter().chain(self.extra_controls) {
            assert_eq!(read(root, path), "owner original", "{path}");
        }
        // The trash is the snapshot's too.
        assert_eq!(
            read(root, ".trash/kept/item.json"),
            "trashed before the backup"
        );
        assert!(!root.join(".trash/since").exists());
    }
}

fn is_ok(outcome: &RestoreOutcome) {
    assert_eq!(
        (outcome.status.as_str(), outcome.reason_code.as_deref()),
        ("ok", None)
    );
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn a_restore_during_setup_makes_the_journal_the_snapshot() {
    let fixture = Fixture::lived_in();
    is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
    fixture.assert_is_the_snapshot();
    assert!(!restore_sweep::is_init_restore(fixture.journal.path()));
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn a_stop_anywhere_ends_the_same() {
    for stop in [
        Stop::BeforeRestore,
        Stop::AfterRestore,
        Stop::ReportAfterRestore,
    ] {
        let fixture = Fixture::lived_in();
        let stopped = fixture.restore(&Restic::stopping(stop));
        assert_eq!(stopped.status, "error");
        assert!(matches!(
            read_whole_journal_restore(fixture.journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));
        is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
        fixture.assert_is_the_snapshot();
        assert!(matches!(
            read_whole_journal_restore(fixture.journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
    }
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn a_failed_restore_or_check_removes_nothing() {
    for stop in [Stop::RestoreFails, Stop::CheckFails] {
        let fixture = Fixture::lived_in();
        let outcome = fixture.restore(&Restic::stopping(stop));
        assert_eq!(outcome.status, "error");
        let root = fixture.journal.path();
        assert_eq!(read(root, "notes/newer.md"), "written since");
        assert!(matches!(
            read_whole_journal_restore(root),
            WholeJournalRestoreReadiness::Pending(_)
        ));
    }
}

#[cfg(unix)]
#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn unsafe_log_selection_never_starts_the_restore_child() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::lived_in();
    let root = fixture.journal.path();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("owner.txt"), "untouched").unwrap();
    fs::rename(root.join("chronicle"), root.join("original-chronicle")).unwrap();
    symlink(outside.path(), root.join("chronicle")).unwrap();
    let runner = Restic::stopping(Stop::Never);
    let outcome = fixture.restore(&runner);
    assert_eq!(outcome.status, "error");
    assert_eq!(runner.restores.get(), 0);
    assert_eq!(read(root, "notes/changed.md"), "edited since");
    assert_eq!(read(root, "notes/newer.md"), "written since");
    assert_eq!(
        fs::read_to_string(outside.path().join("owner.txt")).unwrap(),
        "untouched"
    );
    // Restore the safe namespace before the readiness reader inspects it.
    fs::remove_file(root.join("chronicle")).unwrap();
    fs::rename(root.join("original-chronicle"), root.join("chronicle")).unwrap();
    assert!(restore_sweep::is_init_restore(root));
    is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
    fixture.assert_is_the_snapshot();
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn without_the_marker_a_restore_stays_an_overlay() {
    let fixture = Fixture::lived_in();
    let root = fixture.journal.path();
    restore_sweep::finish_init_restore(root);
    is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
    assert_eq!(read(root, "notes/changed.md"), "as backed up");
    assert_eq!(read(root, "notes/newer.md"), "written since");
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn restoring_an_older_backup_keeps_the_newer_one_through_the_first_prune() {
    let fixture = Fixture::lived_in();
    let root = fixture.journal.path();
    // The journal backs up again as it was lived in: the newest backup.
    fixture.restic(&["backup", &root.display().to_string()]);
    let before = fixture.snapshots();
    assert_eq!(before.len(), 2);
    let newest = before[1].0.clone();

    // The older one is restored, then the restored journal backs up and
    // prunes within the same hour, as its scheduler does in its first minute.
    is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
    fixture.assert_is_the_snapshot();
    solstone_core_backup::set_enabled(root, true).unwrap();
    // The journal's own key opens the repository for its backups and prunes.
    let config = solstone_core_backup::get_backup_config(root).unwrap();
    let daily = tempfile::NamedTempFile::new().unwrap();
    fs::write(daily.path(), config["daily_key"].as_str().unwrap()).unwrap();
    fixture.restic(&[
        "key",
        "add",
        "--new-password-file",
        &daily.path().display().to_string(),
    ]);
    fixture.restic(&["backup", &root.display().to_string()]);
    let clock = TestClock;
    let maintenance = Maintenance;
    let runner = crate::runner::SystemToolRunner;
    let services = BackupServices {
        restic: Some(fixture.binary.as_path().into()),
        ..services(&runner, &clock, &maintenance)
    };
    let pruned = crate::engine::run_prune(root, &services);
    assert_eq!(
        (pruned.status.as_str(), pruned.error_reason.as_deref()),
        ("ok", None)
    );

    let after = fixture.snapshots();
    assert!(
        after.iter().any(|(time, tags)| *time == newest
            && *tags == [crate::engine::KEPT_BEFORE_RESTORE_TAG.to_owned()]),
        "the backup newer than the one restored survives rotation: {after:?}"
    );
    assert_eq!(after.len(), 3, "{after:?}");
}

#[cfg(unix)]
#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn legacy_restore_and_retry_keep_live_authority_and_original_log_writers() {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::MetadataExt;
    for stop in [Stop::Never, Stop::AfterRestore] {
        let fixture = Fixture::lived_in();
        let root = fixture.journal.path();
        let identities: Vec<_> = LEASES
            .iter()
            .map(|p| {
                let m = fs::metadata(root.join(p)).unwrap();
                (m.dev(), m.ino())
            })
            .collect();
        let check_runtime = || {
            for path in AUTHORITY.iter().chain(LEASES) {
                assert_eq!(read(root, path), "destination authority", "{path}");
            }
            for (path, identity) in LEASES.iter().zip(&identities) {
                let m = fs::metadata(root.join(path)).unwrap();
                assert_eq!((m.dev(), m.ino()), *identity, "{path}");
                assert!(
                    acquire_file_lease(
                        root.join(path),
                        LeaseOptions {
                            attempts: 1,
                            ..LeaseOptions::default()
                        }
                    )
                    .unwrap()
                    .is_none(),
                    "a real contender must remain excluded: {path}"
                );
            }
            for log in &fixture.logs {
                log.borrow_mut()
                    .append_bytes_checked(b"destination after restore\n", &|| Ok(()))
                    .unwrap();
            }
            let catalog = catalog_oplogs(
                solstone_core_journal_io::JournalRoot::open(root).unwrap(),
                &[chrono::NaiveDate::from_ymd_opt(2026, 11, 12).unwrap()],
            )
            .unwrap();
            assert_eq!(catalog.entries().len(), 2);
            for (entry, mut file) in catalog.into_catalogued_entries() {
                file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                    .unwrap();
                let mut bytes = String::new();
                file.read_to_string(&mut bytes).unwrap();
                assert!(
                    bytes.contains("destination before restore"),
                    "{:?}",
                    entry.leaf()
                );
                assert!(
                    bytes.contains("destination after restore"),
                    "{:?}",
                    entry.leaf()
                );
                assert!(
                    fixture
                        .logs
                        .iter()
                        .any(|writer| writer.borrow().identity() == entry.identity())
                );
            }
        };
        let outcome = fixture.restore(&Restic::stopping(stop));
        if stop == Stop::AfterRestore {
            assert_eq!(outcome.status, "error");
            // Real copy has changed ordinary owner bytes before this failure.
            assert_eq!(read(root, "notes/changed.md"), "as backed up");
            assert!(matches!(
                read_whole_journal_restore(root),
                WholeJournalRestoreReadiness::Pending(_)
            ));
            check_runtime();
            is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
        } else {
            is_ok(&outcome);
        }
        check_runtime();
        fixture.assert_is_the_snapshot();
        assert!(matches!(
            read_whole_journal_restore(root),
            WholeJournalRestoreReadiness::Absent
        ));
    }
}

#[cfg(unix)]
#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn typed_log_selection_preserves_owner_history_and_sweeps_log_named_owner_entries() {
    use std::os::unix::fs::symlink;
    const LOG: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";
    let fixture = Fixture::lived_in_with_controls(&[
        "chronicle/20261112/health/oplog--not-a-name.log",
        "chronicle/20261112/health/oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl",
        "chronicle/20261112/health/oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a022--daily-think~7df259e6285645a5f9ea769caa484e07.log/owner.txt",
    ]);
    let root = fixture.journal.path();
    let absent = root.join("chronicle/20261112/health").join(LOG);
    fs::create_dir(&absent).unwrap();
    fs::write(absent.join("newer.txt"), "newer owner content").unwrap();
    let external = tempfile::TempDir::new().unwrap();
    let original = external.path().join("owner.txt");
    fs::write(&original, "unchanged").unwrap();
    let link = absent.with_file_name(LOG.replace("8f03cabe", "8f03cabf"));
    symlink(&original, &link).unwrap();
    is_ok(&fixture.restore(&Restic::stopping(Stop::Never)));
    fixture.assert_is_the_snapshot();
    assert!(!absent.exists());
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read_to_string(original).unwrap(), "unchanged");
}

#[cfg(unix)]
#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn late_log_writers_and_private_exclusion_files_survive_until_child_settlement() {
    use std::io::{Read, Seek, SeekFrom};
    for stop in [Stop::Never, Stop::BeforeRestore, Stop::AfterRestore] {
        let fixture = Fixture::lived_in();
        let runner = Restic::stopping(stop);
        runner.make_late_log.set(true);
        let outcome = fixture.restore(&runner);
        assert_eq!(
            outcome.status,
            if stop == Stop::Never { "ok" } else { "error" }
        );
        assert!(!runner.exclusions.borrow().is_empty());
        for path in runner.exclusions.borrow().iter() {
            assert!(
                !path.exists(),
                "exclusion file removed after child: {path:?}"
            );
        }
        let mut late = runner.late_log.borrow_mut();
        let writer = late.as_mut().unwrap();
        writer
            .append_bytes_checked(b"late after restore\n", &|| Ok(()))
            .unwrap();
        let catalog = catalog_oplogs(
            solstone_core_journal_io::JournalRoot::open(fixture.journal.path()).unwrap(),
            &[chrono::NaiveDate::from_ymd_opt(2026, 11, 12).unwrap()],
        )
        .unwrap();
        let (_, mut file) = catalog
            .into_catalogued_entries()
            .into_iter()
            .find(|(entry, _)| entry.identity() == writer.identity())
            .expect("late original writer remains discoverable");
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = String::new();
        file.read_to_string(&mut bytes).unwrap();
        assert!(bytes.contains("late before restore") && bytes.contains("late after restore"));
    }
}
