// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a restore during setup does beyond the restore itself. The real-restic
//! cases are in `native`.

use super::*;

#[cfg(all(test, feature = "full-tests"))]
mod native;

fn write(root: &Path, rel: &str, bytes: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
}

/// Answers `snapshots`, and fails the test if a restore actually runs.
struct RefusesRestore {
    report: ToolOutput,
}

impl ToolRunner for RefusesRestore {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        match request.argv.first().map(|arg| arg.to_string_lossy()) {
            Some(arg) if arg == "snapshots" => Ok(output(0, &catalog())),
            Some(arg) if arg == "restore" && is_dry_run(request) => Ok(self.report.clone()),
            other => panic!("nothing runs after a refusal: {other:?}"),
        }
    }
}

#[test]
fn no_room_refuses_before_anything_changes() {
    let journal = tempfile::tempdir().unwrap();
    let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
    restore_sweep::mark_init_restore(journal.path()).unwrap();
    write(journal.path(), "notes/untouched.md", "untouched");
    let runner = RefusesRestore {
        report: output(
            0,
            &format!(
                "{{\"message_type\":\"summary\",\"bytes_restored\":{}}}\n",
                u64::MAX / 2
            ),
        ),
    };
    let clock = TestClock;
    let maintenance = Maintenance;
    let outcome = restore_journal(
        journal.path(),
        &services(&runner, &clock, &maintenance),
        &Recorder::new(),
        destination(),
        &stored.recovery_key,
    );
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(RESTORE_REASON_INSUFFICIENT_FREE_SPACE)
    );
    assert_eq!(read(journal.path(), "notes/untouched.md"), "untouched");
}

/// Any report is a test failure.
struct NoReports(Script);

impl ToolRunner for NoReports {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        assert!(
            !is_dry_run(request),
            "a restore outside setup reports nothing"
        );
        self.0.run(request)
    }
}

#[test]
fn a_restore_outside_setup_asks_restic_for_no_report() {
    let journal = tempfile::tempdir().unwrap();
    let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
    let runner = NoReports(Script::new(vec![
        output(0, &catalog()),
        output(0, ""),
        output(0, ""),
    ]));
    let clock = TestClock;
    let maintenance = Maintenance;
    let outcome = restore_journal(
        journal.path(),
        &services(&runner, &clock, &maintenance),
        &Recorder::new(),
        destination(),
        &stored.recovery_key,
    );
    assert_ne!(outcome.reason_code.as_deref(), Some("restore_failed"));
}

/// restic answering for a snapshot that holds `notes/from-backup.md`, over a
/// journal whose setup left `notes/setup-only.md`.
struct SetupRestic {
    journal: std::path::PathBuf,
    stop_restore: bool,
    recorded_during_restore: std::cell::Cell<Option<bool>>,
}

impl ToolRunner for SetupRestic {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let argv: Vec<String> = request
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let target = || {
            let at = argv.iter().position(|arg| arg == "--target").unwrap();
            std::path::PathBuf::from(&argv[at + 1])
        };
        match argv.first().map(String::as_str) {
            Some("snapshots") => Ok(output(0, &catalog())),
            Some("check") => Ok(output(0, "")),
            Some("restore") if is_dry_run(request) => {
                let mut report = String::new();
                if argv.iter().any(|arg| arg == "--delete") {
                    let item = target().join("notes").join("setup-only.md");
                    report.push_str(
                        &serde_json::json!({
                            "message_type": "verbose_status",
                            "action": "deleted",
                            "item": item.display().to_string(),
                        })
                        .to_string(),
                    );
                    report.push('\n');
                }
                report.push_str("{\"message_type\":\"summary\",\"bytes_restored\":4}\n");
                Ok(output(0, &report))
            }
            Some("restore") => {
                self.recorded_during_restore
                    .set(Some(obligation_path(&self.journal).is_file()));
                if self.stop_restore {
                    return Err(io::Error::from(io::ErrorKind::Interrupted));
                }
                write(&target(), "notes/from-backup.md", "from backup");
                Ok(output(0, RESTORE_SUMMARY))
            }
            other => panic!("unexpected restic call: {other:?}"),
        }
    }
}

fn restore_during_setup_with(
    stop_restore: bool,
) -> (tempfile::TempDir, RestoreOutcome, SetupRestic) {
    let journal = tempfile::tempdir().unwrap();
    let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
    write(journal.path(), "notes/setup-only.md", "setup");
    let runner = SetupRestic {
        journal: journal.path().to_owned(),
        stop_restore,
        recorded_during_restore: std::cell::Cell::new(None),
    };
    let clock = TestClock;
    let maintenance = Maintenance;
    let outcome = restore_during_setup(
        journal.path(),
        &services(&runner, &clock, &maintenance),
        &Recorder::new(),
        destination(),
        &stored.recovery_key,
    );
    (journal, outcome, runner)
}

#[test]
fn a_setup_restore_runs_on_the_record_and_leaves_only_the_snapshot() {
    let (journal, outcome, runner) = restore_during_setup_with(false);
    assert_eq!(outcome.status, "ok", "{outcome:?}");
    assert_eq!(runner.recorded_during_restore.get(), Some(true));
    assert_eq!(read(journal.path(), "notes/from-backup.md"), "from backup");
    assert!(!journal.path().join("notes/setup-only.md").exists());
    assert!(!restore_sweep::is_init_restore(journal.path()));
    assert_eq!(
        read_whole_journal_restore(journal.path()),
        WholeJournalRestoreReadiness::Absent
    );
}

#[test]
fn a_stopped_setup_restore_keeps_the_journal_paused_at_setup() {
    let (journal, outcome, runner) = restore_during_setup_with(true);
    assert_ne!(outcome.status, "ok", "{outcome:?}");
    assert_eq!(runner.recorded_during_restore.get(), Some(true));
    assert!(matches!(
        read_whole_journal_restore(journal.path()),
        WholeJournalRestoreReadiness::Pending(_)
    ));
    assert!(
        solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(
            journal.path()
        )
    );
    assert!(restore_sweep::is_init_restore(journal.path()));
    assert_eq!(read(journal.path(), "notes/setup-only.md"), "setup");
}
