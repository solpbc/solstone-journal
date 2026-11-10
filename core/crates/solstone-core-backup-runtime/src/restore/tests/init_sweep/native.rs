// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A restore during setup against real restic. Needs `SOLSTONE_NATIVE_RESTIC`.

use super::*;
use crate::runner::run_restic;
use std::cell::Cell;
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
}

impl Restic {
    fn stopping(stop: Stop) -> Self {
        Self {
            stop: Cell::new(stop),
            restores: Cell::new(0),
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

struct Fixture {
    journal: tempfile::TempDir,
    _repository: tempfile::TempDir,
    destination: Destination,
    key: String,
    binary: PathBuf,
}

impl Fixture {
    /// A journal, backed up, then lived in: changed, added to and removed from.
    fn lived_in() -> Self {
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
        restic(&["backup", &root.display().to_string()]);

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
