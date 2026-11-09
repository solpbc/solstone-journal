// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A whole-journal restore makes the journal the snapshot and sets aside what
//! it replaced. The real-restic cases need `SOLSTONE_NATIVE_RESTIC`.

use super::*;
use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const FOLDER_GLOB: &str = ".before-restore-";

fn write(root: &Path, rel: &str, bytes: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
}

fn folder(root: &Path) -> PathBuf {
    let mut found = fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(FOLDER_GLOB))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "{found:?}");
    found.pop().unwrap()
}

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
        let mut child = Command::new(&request.program)
            .arg("--no-cache")
            .args(&request.argv)
            .env_clear()
            .envs(&request.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(bytes) = &request.stdin {
            child.stdin.take().unwrap().write_all(bytes)?;
        }
        let output = child.wait_with_output()?;
        Ok(ToolOutput {
            returncode: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
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
        let repository = tempfile::tempdir().unwrap();
        let mut destination = destination();
        destination.repository = repository.path().join("r").to_string_lossy().into_owned();
        let restic = |args: &[&str]| {
            let status = Command::new(&binary)
                .args(["--no-cache", "-q", "-r", &destination.repository])
                .args(args)
                .env("RESTIC_PASSWORD", &key)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
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

    fn snapshot_id(&self) -> String {
        let output = Command::new(&self.binary)
            .args([
                "--no-cache",
                "-r",
                &self.destination.repository,
                "snapshots",
                "--json",
            ])
            .env("RESTIC_PASSWORD", &self.key)
            .output()
            .unwrap();
        let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
        listed[0]["id"].as_str().unwrap().to_owned()
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
        let clock = TestClock;
        let maintenance = Maintenance;
        let services = BackupServices {
            restic: Some(self.binary.as_path().into()),
            ..services(runner, &clock, &maintenance)
        };
        restore_journal(
            self.journal.path(),
            &services,
            &Recorder::new(),
            self.destination.clone(),
            &self.key,
        )
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

    fn assert_nothing_was_lost(&self) {
        let root = self.journal.path();
        let aside = folder(root);
        assert_eq!(read(&aside, "notes/changed.md"), "edited since");
        assert_eq!(read(&aside, "notes/newer.md"), "written since");
        assert_eq!(
            read(&aside, "notes/deep/er/newest.md"),
            "written since, deeper"
        );
        assert_eq!(read(&aside, "config/removals/newer.json"), "newer entry");
        assert_eq!(
            read(&aside, "chronicle/20260101/s/090000_300/tombstone.json"),
            "{}"
        );
        assert_eq!(
            read(&aside, "entities/cache/entity.json"),
            "changed owner cache"
        );
        assert_eq!(
            read(&aside, ".trash/since/item.json"),
            "trashed since the backup"
        );
        assert!(!aside.join("notes/same.md").exists(), "unchanged stays out");
        assert!(
            !aside.join("cache").exists(),
            "what a restore leaves alone stays out"
        );
    }
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn the_journal_becomes_the_snapshot_and_nothing_is_lost() {
    let fixture = Fixture::lived_in();
    let outcome = fixture.restore(&Restic::stopping(Stop::Never));
    assert_eq!(
        (outcome.status.as_str(), outcome.reason_code.as_deref()),
        ("ok", None)
    );
    fixture.assert_is_the_snapshot();
    fixture.assert_nothing_was_lost();
    assert!(
        !fixture
            .journal
            .path()
            .join(restore_set_aside::ATTEMPT_FILE)
            .exists()
    );
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn a_stop_anywhere_ends_the_same_and_keeps_the_true_original() {
    for stop in [
        Stop::BeforeRestore,
        Stop::AfterRestore,
        Stop::ReportAfterRestore,
    ] {
        let fixture = Fixture::lived_in();
        fixture.restore(&Restic::stopping(stop));
        // Whatever happened, the first copy of the changed file is the original.
        assert_eq!(
            read(&folder(fixture.journal.path()), "notes/changed.md"),
            "edited since"
        );
        let outcome = fixture.restore(&Restic::stopping(Stop::Never));
        assert_eq!(
            (outcome.status.as_str(), outcome.reason_code.as_deref()),
            ("ok", None)
        );
        fixture.assert_is_the_snapshot();
        fixture.assert_nothing_was_lost();
    }
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn a_failed_restore_or_check_removes_nothing() {
    for stop in [Stop::RestoreFails, Stop::CheckFails] {
        let fixture = Fixture::lived_in();
        let outcome = fixture.restore(&Restic::stopping(stop));
        assert_ne!(outcome.status, "ok");
        let root = fixture.journal.path();
        assert_eq!(read(root, "notes/newer.md"), "written since");
        assert_eq!(read(root, "config/removals/newer.json"), "newer entry");
    }
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn a_report_that_fails_after_the_restore_never_reads_as_success() {
    let fixture = Fixture::lived_in();
    let outcome = fixture.restore(&Restic::stopping(Stop::ReportAfterRestore));
    assert_eq!(outcome.status, "degraded");
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(RESTORE_REASON_SET_ASIDE_INCOMPLETE)
    );
    assert_eq!(
        read(fixture.journal.path(), "notes/newer.md"),
        "written since"
    );
}

#[test]
#[ignore = "requires native restic; set SOLSTONE_NATIVE_RESTIC"]
fn on_mobile_a_failed_restore_keeps_its_obligation_and_the_retry_finishes() {
    let fixture = Fixture::lived_in();
    let root = fixture.journal.path();
    let drained = Cell::new(0);
    let failed = fixture.restore_mobile(&Restic::stopping(Stop::RestoreFails), &drained);
    assert_eq!(failed.status, "error");
    assert_eq!(drained.get(), 1, "the copies are made after the drain");
    assert!(matches!(
        read_whole_journal_restore(root),
        WholeJournalRestoreReadiness::Pending(_)
    ));
    assert_eq!(read(&folder(root), "notes/changed.md"), "edited since");
    assert_eq!(read(root, "notes/newer.md"), "written since");

    let retried = fixture.restore_mobile(&Restic::stopping(Stop::Never), &drained);
    assert_eq!(
        (retried.status.as_str(), retried.reason_code.as_deref()),
        ("ok", None)
    );
    assert!(matches!(
        read_whole_journal_restore(root),
        WholeJournalRestoreReadiness::Absent
    ));
    fixture.assert_is_the_snapshot();
    fixture.assert_nothing_was_lost();
}

/// Answers `snapshots` and fails the test if a restore actually runs.
struct RefusesRestore {
    report: ToolOutput,
}

impl ToolRunner for RefusesRestore {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        match request.argv.first().map(|arg| arg.to_string_lossy()) {
            Some(arg) if arg == "snapshots" => Ok(output(0, &catalog())),
            Some(arg) if arg == "restore" && is_dry_run(request) => Ok(self.report.clone()),
            other => panic!("nothing runs after a refused report: {other:?}"),
        }
    }
}

fn refused_with(report: ToolOutput) -> (RestoreOutcome, tempfile::TempDir) {
    let journal = tempfile::tempdir().unwrap();
    let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
    write(journal.path(), "notes/changed.md", "untouched");
    let runner = RefusesRestore { report };
    let clock = TestClock;
    let maintenance = Maintenance;
    let outcome = restore_journal(
        journal.path(),
        &services(&runner, &clock, &maintenance),
        &Recorder::new(),
        destination(),
        &stored.recovery_key,
    );
    (outcome, journal)
}

#[test]
fn a_report_that_is_not_whole_refuses_before_anything_changes() {
    for report in [
        output(1, ""),
        output(0, "{\"message_type\":\"status\"}\n"),
        output(0, "torn"),
    ] {
        let (outcome, journal) = refused_with(report);
        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_PREVIEW_FAILED)
        );
        assert_eq!(read(journal.path(), "notes/changed.md"), "untouched");
        assert!(
            !journal
                .path()
                .join(restore_set_aside::ATTEMPT_FILE)
                .exists()
        );
    }
}

#[test]
fn on_mobile_a_refused_report_leaves_the_journal_running() {
    let journal = tempfile::tempdir().unwrap();
    let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
    let runner = RefusesRestore {
        report: output(1, ""),
    };
    let clock = TestClock;
    let maintenance = Maintenance;
    let mut drain = || -> Result<(), String> { panic!("a refused report must not drain") };
    let outcome = restore_mobile_journal(
        Arc::new(solstone_core_journal_io::JournalRoot::open(journal.path()).unwrap()),
        &services(&runner, &clock, &maintenance),
        &Recorder::new(),
        destination(),
        &stored.recovery_key,
        MobileRestoreIdentity::Byo,
        MobileSelection {
            snapshot_id: SNAPSHOT_ID.into(),
            subtree: "/journal".into(),
        },
        &mut drain,
    );
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(RESTORE_REASON_PREVIEW_FAILED)
    );
    assert!(matches!(
        read_whole_journal_restore(journal.path()),
        WholeJournalRestoreReadiness::Absent
    ));
}

#[test]
fn no_room_refuses_before_anything_changes() {
    let (outcome, journal) = refused_with(output(
        0,
        &format!(
            "{{\"message_type\":\"summary\",\"bytes_restored\":{}}}\n",
            u64::MAX / 2
        ),
    ));
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(RESTORE_REASON_INSUFFICIENT_FREE_SPACE)
    );
    assert_eq!(read(journal.path(), "notes/changed.md"), "untouched");
    let hidden = fs::read_dir(journal.path())
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with(FOLDER_GLOB));
    assert!(!hidden, "no set-aside folder");
}

#[test]
fn a_newer_entry_in_the_removal_record_leaves_with_the_rest() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "config/removals/newer.json", "newer");
    let report = restore_set_aside::Report {
        deleted: vec!["config/removals/newer.json".into()],
        ..restore_set_aside::Report::default()
    };
    assert_eq!(
        restore_set_aside::move_removed(root.path(), ".before-restore-1", &report),
        0
    );
    assert!(!root.path().join("config/removals/newer.json").exists());
    assert_eq!(
        read(
            &root.path().join(".before-restore-1"),
            "config/removals/newer.json"
        ),
        "newer"
    );
}

#[test]
fn a_file_that_cannot_move_means_the_restore_is_not_finished() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "notes/stuck.md", "held");
    // The folder's place for it is blocked by a file where a directory goes.
    write(root.path(), ".before-restore-1/notes", "in the way");
    let report = restore_set_aside::Report {
        deleted: vec!["notes/stuck.md".into()],
        ..restore_set_aside::Report::default()
    };
    assert_eq!(
        restore_set_aside::move_removed(root.path(), ".before-restore-1", &report),
        1
    );
    assert_eq!(read(root.path(), "notes/stuck.md"), "held");
}
