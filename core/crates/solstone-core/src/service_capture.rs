// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Self-capture for a supervisor directly exec'd by systemd or launchd.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local, NaiveDate};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};

use crate::service::current_process_has_matching_installation_guard;

const ROLLOVER_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Keeps a valid installed-service capture active for the supervisor lifetime.
pub(super) struct ServiceCapture {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for ServiceCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Redirect this process only when its inherited installation guard fully
/// matches the saved binding. An unguarded manual `journal start` stays on its
/// inherited stdout and stderr exactly as before.
pub(super) fn start_if_guarded(journal: &Path) -> Result<Option<ServiceCapture>, String> {
    start_if_guarded_with(
        journal,
        current_process_has_matching_installation_guard(),
        start_capture,
    )
}

fn start_if_guarded_with(
    journal: &Path,
    guard_matches: bool,
    start: impl FnOnce(&Path) -> Result<ServiceCapture, String>,
) -> Result<Option<ServiceCapture>, String> {
    if !guard_matches {
        return Ok(None);
    }
    start(journal).map(Some)
}

fn start_capture(journal: &Path) -> Result<ServiceCapture, String> {
    let opened = local_instant();
    let writer = open_and_redirect(journal, opened, open_service_oplog, redirect_both)?;

    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let journal = journal.to_path_buf();
    let worker =
        thread::spawn(move || rollover_worker(journal, writer, opened.date_naive(), worker_stop));
    Ok(ServiceCapture {
        stop,
        worker: Some(worker),
    })
}

fn rollover_worker(
    journal: PathBuf,
    mut writer: OplogWriter,
    mut day: NaiveDate,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(ROLLOVER_POLL_INTERVAL);
        let _ = rollover_at_with(
            &mut writer,
            &mut day,
            local_instant(),
            |opened| open_service_oplog(&journal, opened),
            redirect_both,
        );
    }
}

fn local_instant() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn open_service_oplog(
    journal: &Path,
    opened: DateTime<FixedOffset>,
) -> Result<OplogWriter, String> {
    let root = JournalRoot::open(journal)
        .map_err(|error| format!("open journal for service capture: {error}"))?;
    create_oplog_at(root, "service", "supervisor", OplogFormat::Log, opened)
        .map_err(|error| format!("create service capture oplog: {error}"))
}

fn open_and_redirect(
    journal: &Path,
    opened: DateTime<FixedOffset>,
    open: impl FnOnce(&Path, DateTime<FixedOffset>) -> Result<OplogWriter, String>,
    redirect: impl FnOnce(&OplogWriter, Option<&OplogWriter>) -> io::Result<()>,
) -> Result<OplogWriter, String> {
    let writer = open(journal, opened)?;
    redirect(&writer, None).map_err(|error| format!("redirect service capture: {error}"))?;
    Ok(writer)
}

fn rollover_at_with(
    writer: &mut OplogWriter,
    day: &mut NaiveDate,
    opened: DateTime<FixedOffset>,
    open: impl FnOnce(DateTime<FixedOffset>) -> Result<OplogWriter, String>,
    redirect: impl FnOnce(&OplogWriter, Option<&OplogWriter>) -> io::Result<()>,
) -> bool {
    if opened.date_naive() == *day {
        return false;
    }
    let Ok(next) = open(opened) else {
        return false;
    };
    if redirect(&next, Some(writer)).is_err() {
        return false;
    }
    *writer = next;
    *day = opened.date_naive();
    true
}

fn redirect_both(next: &OplogWriter, previous: Option<&OplogWriter>) -> io::Result<()> {
    redirect_both_with(next, previous, duplicate_stdout, duplicate_stderr)
}

fn redirect_both_with(
    next: &OplogWriter,
    previous: Option<&OplogWriter>,
    duplicate_stdout: impl Fn(&OplogWriter) -> io::Result<()>,
    duplicate_stderr: impl Fn(&OplogWriter) -> io::Result<()>,
) -> io::Result<()> {
    duplicate_stdout(next)?;
    if let Err(error) = duplicate_stderr(next) {
        if let Some(previous) = previous {
            let _ = duplicate_stdout(previous);
        }
        return Err(error);
    }
    Ok(())
}

fn duplicate_stdout(writer: &OplogWriter) -> io::Result<()> {
    // `dup2` is the required process-wide redirection primitive: library code
    // that writes directly to fd 1 or 2 cannot otherwise participate.
    nix::unistd::dup2_stdout(writer).map_err(io::Error::from)
}

fn duplicate_stderr(writer: &OplogWriter) -> io::Result<()> {
    nix::unistd::dup2_stderr(writer).map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::io::Write;
    use std::process::Command;

    use chrono::TimeZone;

    use super::*;

    fn instant(day: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, day, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn open_test_writer(journal: &Path, opened: DateTime<FixedOffset>) -> OplogWriter {
        open_service_oplog(journal, opened).unwrap()
    }

    fn leaf_path(journal: &Path, opened: DateTime<FixedOffset>, writer: &OplogWriter) -> PathBuf {
        journal
            .join("chronicle")
            .join(opened.format("%Y%m%d").to_string())
            .join("health")
            .join(writer.leaf_name())
    }

    fn assert_contains(path: &Path, expected: &[u8]) {
        assert!(
            fs::read(path)
                .unwrap()
                .windows(expected.len())
                .any(|bytes| bytes == expected),
            "{} did not contain {:?}",
            path.display(),
            expected
        );
    }

    #[test]
    fn false_guard_short_circuits_before_capture_start() {
        let result = start_if_guarded_with(Path::new("unused"), false, |_| {
            Err("capture must not be opened".to_owned())
        });
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn initial_capture_creation_or_redirect_failure_is_fatal() {
        let journal = tempfile::tempdir().unwrap();
        let creation = open_and_redirect(
            journal.path(),
            instant(7),
            |_, _| Err("injected oplog create failure".to_owned()),
            |_, _| Ok(()),
        )
        .unwrap_err();
        assert!(creation.contains("injected oplog create failure"));

        let redirect = open_and_redirect(journal.path(), instant(7), open_service_oplog, |_, _| {
            Err(io::Error::other("injected dup2 failure"))
        })
        .unwrap_err();
        assert!(redirect.contains("redirect service capture: injected dup2 failure"));
    }

    #[test]
    fn redirection_writes_both_targets_to_the_service_oplog() {
        const CHILD_JOURNAL: &str = "SOLSTONE_SERVICE_CAPTURE_CHILD_JOURNAL";
        if let Ok(journal) = std::env::var(CHILD_JOURNAL) {
            let journal = PathBuf::from(journal);
            let opened = instant(7);
            let writer = open_test_writer(&journal, opened);
            fs::write(journal.join("capture-leaf"), writer.leaf_name()).unwrap();
            redirect_both(&writer, None).unwrap();
            nix::unistd::write(std::io::stdout(), b"stdout bytes").unwrap();
            nix::unistd::write(std::io::stderr(), b"stderr bytes").unwrap();
            return;
        }

        let journal = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service_capture::tests::redirection_writes_both_targets_to_the_service_oplog",
                "--nocapture",
            ])
            .env(CHILD_JOURNAL, journal.path())
            .status()
            .unwrap();

        assert!(status.success());
        let leaf = fs::read_to_string(journal.path().join("capture-leaf")).unwrap();
        let path = journal.path().join("chronicle/20260807/health").join(leaf);
        assert_contains(&path, b"stdout bytes");
        assert_contains(&path, b"stderr bytes");
    }

    #[test]
    fn rollover_create_or_second_redirect_failure_retains_the_old_destination() {
        let journal = tempfile::tempdir().unwrap();
        let opened = instant(7);
        let mut writer = open_test_writer(journal.path(), opened);
        let old_path = leaf_path(journal.path(), opened, &writer);
        let mut day = opened.date_naive();

        assert!(!rollover_at_with(
            &mut writer,
            &mut day,
            instant(8),
            |_| Err("injected rollover create failure".to_owned()),
            |_, _| unreachable!("a failed create must not redirect"),
        ));
        writer.write_all(b"after create failure").unwrap();
        writer.flush().unwrap();
        assert_contains(&old_path, b"after create failure");

        assert!(!rollover_at_with(
            &mut writer,
            &mut day,
            instant(8),
            |next_opened| Ok(open_test_writer(journal.path(), next_opened)),
            |next, previous| {
                redirect_both_with(
                    next,
                    previous,
                    |_| Ok(()),
                    |_| Err(io::Error::other("injected second redirect failure")),
                )
            },
        ));
        writer.write_all(b"after redirect failure").unwrap();
        writer.flush().unwrap();
        assert_contains(&old_path, b"after redirect failure");
        assert_eq!(writer.leaf_name(), old_path.file_name().unwrap());
    }

    #[test]
    fn rollover_swaps_only_after_both_targets_redirect_to_the_new_oplog() {
        let journal = tempfile::tempdir().unwrap();
        let opened = instant(7);
        let mut writer = open_test_writer(journal.path(), opened);
        let old_path = leaf_path(journal.path(), opened, &writer);
        writer.write_all(b"before rollover").unwrap();

        let mut day = opened.date_naive();
        let redirects = RefCell::new(Vec::new());
        assert!(rollover_at_with(
            &mut writer,
            &mut day,
            instant(8),
            |next_opened| Ok(open_test_writer(journal.path(), next_opened)),
            |next, previous| {
                assert!(previous.is_some());
                redirect_both_with(
                    next,
                    previous,
                    |_| {
                        redirects.borrow_mut().push("stdout");
                        Ok(())
                    },
                    |_| {
                        redirects.borrow_mut().push("stderr");
                        Ok(())
                    },
                )
            },
        ));
        assert_eq!(redirects.into_inner(), ["stdout", "stderr"]);
        let new_path = leaf_path(journal.path(), instant(8), &writer);
        writer.write_all(b"after rollover").unwrap();
        writer.flush().unwrap();

        assert_contains(&old_path, b"before rollover");
        assert!(
            !fs::read(&old_path)
                .unwrap()
                .windows(b"after rollover".len())
                .any(|bytes| bytes == b"after rollover")
        );
        assert_contains(&new_path, b"after rollover");
        assert_eq!(day, instant(8).date_naive());
    }
}
