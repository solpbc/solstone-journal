// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical operational-log follow driver layered over journal-I/O's injected
//! identity follower.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{
        LeaseProbe, OplogCatalogEntry, OplogCatalogError, OplogClock, OplogEntryReaderFactory,
        OplogFollowReader, OplogFollowTickOutcome, OplogFollower, OplogIdentityProbe,
        OplogSnapshotSource, catalog_oplogs, open_oplog_catalog_entry,
        probe_oplog_catalog_entry_lease,
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

struct CatalogReaderFactory {
    root: PathBuf,
}

impl OplogEntryReaderFactory for CatalogReaderFactory {
    fn open(&self, entry: &OplogCatalogEntry) -> io::Result<Box<dyn OplogFollowReader>> {
        let root =
            JournalRoot::open(&self.root).map_err(|error| io::Error::other(error.to_string()))?;
        let mut file = open_oplog_catalog_entry(root, entry)
            .map_err(|error| io::Error::other(error.to_string()))?;
        file.seek(SeekFrom::Start(entry.payload_offset() as u64))?;
        Ok(Box::new(CatalogReader(BufReader::new(file))))
    }
}

struct CatalogReader(BufReader<File>);

impl OplogFollowReader for CatalogReader {
    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        if self.0.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        } else if line.ends_with('\r') {
            line.pop();
        }
        Ok(Some(line))
    }

    fn seek_to_end(&mut self) -> io::Result<()> {
        self.0.seek(SeekFrom::End(0)).map(|_| ())
    }
}

struct CatalogProbe {
    root: PathBuf,
}

impl OplogIdentityProbe for CatalogProbe {
    fn probe(&self, entry: &OplogCatalogEntry) -> LeaseProbe {
        let Ok(root) = JournalRoot::open(&self.root) else {
            return LeaseProbe::Indeterminate;
        };
        probe_oplog_catalog_entry_lease(root, entry)
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
    let source = CatalogSource {
        root: journal_root.to_path_buf(),
    };
    let factory = CatalogReaderFactory {
        root: journal_root.to_path_buf(),
    };
    let probe = CatalogProbe {
        root: journal_root.to_path_buf(),
    };
    let clock = LocalClock;
    let initial = OplogFollower::discover_initial(&source, &factory, &clock)
        .map_err(|_| fatal(journal_root, "catalog"))?;
    if !initial.has_tracked_sources {
        warn("No log files found.".to_owned());
        return Ok(());
    }
    let mut follower = OplogFollower::from_state(initial.state);
    let mut last_service = None;
    while !stop() {
        let mut output_failure = None;
        let outcome = follower.tick(
            &source,
            &factory,
            &probe,
            &clock,
            stop,
            &mut |entry, line| {
                if output_failure.is_none()
                    && let Err(error) = render_stream_row(
                        output,
                        &line,
                        Some(entry.name().source().display_slug()),
                        is_tty,
                        &mut last_service,
                    )
                {
                    output_failure = Some(error);
                }
            },
        );
        if let Some(error) = output_failure {
            return Err(output_error(error));
        }
        let outcome = outcome.map_err(|_| fatal(journal_root, "catalog"))?;
        output.flush().map_err(output_error)?;
        if outcome == OplogFollowTickOutcome::Stopped {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(())
}

fn fatal(path: &Path, operation: &'static str) -> FollowFatalError {
    FollowFatalError {
        path: path.to_path_buf(),
        operation,
        source: None,
    }
}

fn output_error(source: io::Error) -> FollowFatalError {
    FollowFatalError {
        path: PathBuf::from("<stdout>"),
        operation: "output",
        source: Some(source),
    }
}
