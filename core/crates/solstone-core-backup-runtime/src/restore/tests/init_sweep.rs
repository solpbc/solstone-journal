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
