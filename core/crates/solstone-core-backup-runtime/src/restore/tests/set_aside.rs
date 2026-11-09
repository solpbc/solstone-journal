// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A whole-journal restore makes the journal the snapshot and sets aside what
//! it replaced. The real-restic cases are in `native`.

use super::*;

#[cfg(all(test, feature = "full-tests"))]
mod native;

const FOLDER_GLOB: &str = ".before-restore-";

fn write(root: &Path, rel: &str, bytes: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|error| panic!("{rel}: {error}"))
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
