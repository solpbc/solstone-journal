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

const NEWER_ID: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const ARCHIVE_ID: &str = "abababababababababababababababababababababababababababababababab";
const OFFLOAD_ID: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// The restored snapshot, an archive and offloaded media newer than it, and
/// optionally a journal backup newer than it.
fn catalog_after(with_newer: bool) -> String {
    let mut entries = vec![
        format!(
            "{{\"id\":\"{SNAPSHOT_ID}\",\"time\":\"2026-01-01T00:00:00Z\",\"paths\":[\"/journal\"]}}"
        ),
        format!(
            "{{\"id\":\"{ARCHIVE_ID}\",\"time\":\"2026-01-01T00:30:00Z\",\"paths\":[\"/journal/chronicle\"],\"tags\":[\"solstone-archive\"]}}"
        ),
        format!(
            "{{\"id\":\"{OFFLOAD_ID}\",\"time\":\"2026-01-01T00:40:00Z\",\"paths\":[\"/journal/chronicle\"],\"tags\":[\"spb-offload-archive\"]}}"
        ),
    ];
    if with_newer {
        entries.push(format!(
            "{{\"id\":\"{NEWER_ID}\",\"time\":\"2026-01-01T00:20:00Z\",\"paths\":[\"/journal\"]}}"
        ));
    }
    format!("[{}]", entries.join(","))
}

/// Answers the catalog and the free-space report, records every other call,
/// answers `tag` with `tag_code`, and fails the test if a restore copies.
struct KeepNewer {
    with_newer: bool,
    tag_code: i32,
    calls: RefCell<Vec<Vec<String>>>,
}

impl ToolRunner for KeepNewer {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let argv: Vec<String> = request
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        match argv.first().map(String::as_str) {
            Some("snapshots") => Ok(output(0, &catalog_after(self.with_newer))),
            Some("restore") if is_dry_run(request) => Ok(empty_report()),
            Some("unlock") => {
                self.calls.borrow_mut().push(argv);
                Ok(output(0, ""))
            }
            Some("tag") => {
                self.calls.borrow_mut().push(argv);
                Ok(output(self.tag_code, ""))
            }
            other => panic!("this test stops before the copy: {other:?}"),
        }
    }
}

fn restore_older(runner: &KeepNewer) -> (tempfile::TempDir, RestoreOutcome) {
    let journal = tempfile::tempdir().unwrap();
    let stored = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
    restore_sweep::mark_init_restore(journal.path()).unwrap();
    write(journal.path(), "notes/untouched.md", "untouched");
    let clock = TestClock;
    let maintenance = Maintenance;
    let outcome = restore_mobile_journal(
        Arc::new(solstone_core_journal_io::JournalRoot::open(journal.path()).unwrap()),
        &services(runner, &clock, &maintenance),
        &Recorder::new(),
        destination(),
        &stored.recovery_key,
        MobileRestoreIdentity::Byo,
        MobileSelection {
            snapshot_id: SNAPSHOT_ID.to_owned(),
            subtree: "/journal".to_owned(),
        },
        &mut || Err("stop before the copy".to_owned()),
    );
    (journal, outcome)
}

#[test]
fn restoring_an_older_backup_keeps_only_the_newer_journal_backups() {
    let runner = KeepNewer {
        with_newer: true,
        tag_code: 0,
        calls: RefCell::new(Vec::new()),
    };
    restore_older(&runner);
    let calls = runner.calls.borrow();
    assert_eq!(calls[0], ["unlock"], "stale locks clear first: {calls:?}");
    let tag = &calls[1];
    assert_eq!(&tag[..3], ["tag", "--add", "solstone-kept-before-restore"]);
    assert!(
        tag.windows(2).any(|pair| pair[0] == "--retry-lock"),
        "{tag:?}"
    );
    assert!(tag.contains(&NEWER_ID.to_owned()), "{tag:?}");
    for untouched in [SNAPSHOT_ID, ARCHIVE_ID, OFFLOAD_ID] {
        assert!(
            !tag.contains(&untouched.to_owned()),
            "{untouched} keeps its id: {tag:?}"
        );
    }
}

#[test]
fn restoring_the_newest_backup_asks_restic_for_nothing_more() {
    let runner = KeepNewer {
        with_newer: false,
        tag_code: 0,
        calls: RefCell::new(Vec::new()),
    };
    restore_older(&runner);
    assert!(
        runner.calls.borrow().is_empty(),
        "{:?}",
        runner.calls.borrow()
    );
}

#[test]
fn a_locked_repository_stops_the_restore_before_it_is_recorded_or_copied() {
    let runner = KeepNewer {
        with_newer: true,
        tag_code: 11,
        calls: RefCell::new(Vec::new()),
    };
    let (journal, outcome) = restore_older(&runner);
    assert_eq!(
        outcome.reason_code.as_deref(),
        Some(crate::runner::reason_for_returncode(11))
    );
    assert!(
        matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ),
        "nothing recorded, so the owner can still pick another date"
    );
    assert_eq!(read(journal.path(), "notes/untouched.md"), "untouched");
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
            // Bookkeeping around the restore (repository unlock, tagging newer backups)
            // has nothing to report here.
            Some("check" | "unlock" | "tag") => Ok(output(0, "")),
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
