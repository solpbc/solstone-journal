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
    let writer = open_service_oplog(journal, opened)?;
    redirect_both(&writer, None).map_err(|error| format!("redirect service capture: {error}"))?;

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
        let opened = local_instant();
        if opened.date_naive() == day {
            continue;
        }
        let Ok(next) = open_service_oplog(&journal, opened) else {
            continue;
        };
        if redirect_both(&next, Some(&writer)).is_ok() {
            writer = next;
            day = opened.date_naive();
        }
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

fn redirect_both(next: &OplogWriter, previous: Option<&OplogWriter>) -> io::Result<()> {
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
    use super::*;

    #[test]
    fn false_guard_short_circuits_before_capture_start() {
        let result = start_if_guarded_with(Path::new("unused"), false, |_| {
            Err("capture must not be opened".to_owned())
        });
        assert!(result.unwrap().is_none());
    }
}
