// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical operational-log follow driver layered over journal-I/O's
//! descriptor-bound transactional follower.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{
        OplogCatalogError, OplogClock, OplogFollower, OplogSnapshotSource, catalog_oplogs,
    },
};

use crate::render::render_stream_row;

/// Fatal canonical-follow failure with a safe source path for CLI diagnostics.
#[derive(Debug)]
pub struct FollowFatalError {
    pub path: PathBuf,
    pub operation: &'static str,
    pub source: Option<io::Error>,
}

struct CatalogSource {
    root: PathBuf,
}

impl OplogSnapshotSource for CatalogSource {
    fn snapshot(
        &self,
        days: &[NaiveDate],
    ) -> Result<solstone_core_journal_io::operational_log::OplogCatalogSnapshot, OplogCatalogError>
    {
        let root = JournalRoot::open(&self.root).map_err(|_| OplogCatalogError::root())?;
        catalog_oplogs(root, days)
    }
}

struct LocalClock;

impl OplogClock for LocalClock {
    fn today(&self) -> NaiveDate {
        Local::now().date_naive()
    }
}

/// Run canonical follow until `stop` becomes true.
pub fn run_follow(
    journal_root: &Path,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
    warn: &mut dyn FnMut(String),
) -> Result<(), FollowFatalError> {
    run_follow_with_sleep(
        journal_root,
        stop,
        output,
        is_tty,
        warn,
        &std::thread::sleep,
    )
}

fn run_follow_with_sleep(
    journal_root: &Path,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
    warn: &mut dyn FnMut(String),
    sleep: &dyn Fn(std::time::Duration),
) -> Result<(), FollowFatalError> {
    let source = CatalogSource {
        root: journal_root.to_path_buf(),
    };
    let clock = LocalClock;
    let initial = OplogFollower::discover_initial(&source, &clock)
        .map_err(|error| catalog_error(journal_root, error))?;
    if !initial.has_tracked_sources {
        warn("No log files found.".to_owned());
        return Ok(());
    }
    let mut follower = OplogFollower::from_state(initial.state);
    let mut last_service = None;
    while !stop() {
        let tick = follower
            .tick(&source, &clock, stop)
            .map_err(|error| catalog_error(journal_root, error))?;
        let Some(rows) = tick.into_rows() else {
            return Ok(());
        };
        for (entry, line) in rows {
            render_stream_row(
                output,
                &line,
                Some(entry.name().source().display_slug()),
                is_tty,
                &mut last_service,
            )
            .map_err(output_error)?;
            output.flush().map_err(output_error)?;
        }
        sleep(std::time::Duration::from_millis(200));
    }
    Ok(())
}

fn catalog_error(path: &Path, error: OplogCatalogError) -> FollowFatalError {
    FollowFatalError {
        path: path.to_path_buf(),
        operation: "catalog",
        source: Some(io::Error::other(error.to_string())),
    }
}

fn output_error(source: io::Error) -> FollowFatalError {
    FollowFatalError {
        path: PathBuf::from("<stdout>"),
        operation: "output",
        source: Some(source),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use solstone_core_journal_io::operational_log::{OplogFormat, create_oplog};

    use super::*;

    struct FlushFailure {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Err(io::Error::other("injected flush failure"))
        }
    }

    #[test]
    fn flush_failure_returns_before_later_rows_or_sleep() {
        let temporary = tempfile::tempdir().unwrap();
        for source in ["alpha", "beta"] {
            let mut writer = create_oplog(
                JournalRoot::open(temporary.path()).unwrap(),
                source,
                "follow-output-test",
                OplogFormat::Log,
            )
            .unwrap();
            writeln!(writer, "{source}-sentinel").unwrap();
        }
        let mut output = FlushFailure {
            bytes: Vec::new(),
            flushes: 0,
        };
        let sleeps = Cell::new(0);
        let error = run_follow_with_sleep(
            temporary.path(),
            &|| false,
            &mut output,
            false,
            &mut |_| {},
            &|_| sleeps.set(sleeps.get() + 1),
        )
        .unwrap_err();
        assert_eq!(error.operation, "output");
        assert_eq!(output.flushes, 1);
        assert_eq!(
            output.bytes.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
        assert_eq!(sleeps.get(), 0);
    }
}
