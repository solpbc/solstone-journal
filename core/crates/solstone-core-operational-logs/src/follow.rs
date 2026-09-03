// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical operational-log follow driver layered over journal-I/O's
//! descriptor-bound transactional follower.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{
        OplogCatalogEntry, OplogCatalogError, OplogClock, OplogFollower, OplogFormat,
        OplogSnapshotSource, catalog_oplogs,
    },
};

use crate::SourceTailSnapshot;
use crate::render::{render_raw_stream, render_stream_row};

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

/// Follow one raw `.log` source after emitting a [`SourceTailSnapshot`].
///
/// The snapshot's retained descriptor frontiers become the follower's
/// committed offsets, so bytes already rendered are not replayed and later
/// bytes are not skipped. Unlike [`run_follow`], this path intentionally does
/// not impose UTF-8, newline, or record-size constraints on captured process
/// output.
pub fn run_follow_from_snapshot(
    snapshot: SourceTailSnapshot,
    journal_root: &Path,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
) -> Result<(), FollowFatalError> {
    run_follow_from_snapshot_with_sleep(
        snapshot,
        journal_root,
        stop,
        output,
        is_tty,
        &std::thread::sleep,
        &|| Local::now().date_naive(),
    )
}

fn run_follow_from_snapshot_with_sleep(
    snapshot: SourceTailSnapshot,
    journal_root: &Path,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
    sleep: &dyn Fn(std::time::Duration),
    today: &dyn Fn() -> NaiveDate,
) -> Result<(), FollowFatalError> {
    let source_slug = snapshot.source_slug().to_owned();
    let mut follower = RawOplogFollower::from_catalogued_frontiers(
        source_slug,
        snapshot.into_catalogued_frontiers(),
    )?;
    while !stop() {
        for bytes in follower.tick(journal_root, today())? {
            render_raw_stream(output, &bytes, is_tty).map_err(output_error)?;
            output.flush().map_err(output_error)?;
        }
        sleep(std::time::Duration::from_millis(200));
    }
    Ok(())
}

struct RawTrackedOplog {
    entry: OplogCatalogEntry,
    file: File,
    committed_frontier: u64,
}

/// Byte-oriented follower for one raw captured-output source.
///
/// OplogWriter leases and admission make descriptors immutable identities; raw
/// capture is allowed to expose every appended byte before the writer closes.
struct RawOplogFollower {
    source_slug: String,
    tracked: Vec<RawTrackedOplog>,
}

impl RawOplogFollower {
    fn from_catalogued_frontiers(
        source_slug: String,
        entries: Vec<(OplogCatalogEntry, File, u64)>,
    ) -> Result<Self, FollowFatalError> {
        let mut tracked = Vec::new();
        for (entry, file, frontier) in entries {
            if !matches_raw_source(&entry, &source_slug) {
                continue;
            }
            let current = file
                .metadata()
                .map_err(|error| raw_catalog_error(&entry, error))?
                .len();
            if frontier < entry.payload_offset() as u64 || frontier > current {
                return Err(raw_catalog_identity_error(&entry));
            }
            tracked.push(RawTrackedOplog {
                entry,
                file,
                committed_frontier: frontier,
            });
        }
        Ok(Self {
            source_slug,
            tracked,
        })
    }

    fn tick(
        &mut self,
        journal_root: &Path,
        today: NaiveDate,
    ) -> Result<Vec<Vec<u8>>, FollowFatalError> {
        let days = raw_follow_days(today);
        self.tracked.retain(|tracked| {
            days.iter()
                .any(|day| day.format("%Y%m%d").to_string() == tracked.entry.day())
        });

        let mut appended = Vec::new();
        for tracked in &mut self.tracked {
            if let Some(bytes) = read_appended_bytes(tracked)? {
                appended.push(bytes);
            }
        }

        let root = JournalRoot::open(journal_root).map_err(|_| FollowFatalError {
            path: journal_root.to_path_buf(),
            operation: "catalog",
            source: Some(io::Error::other("journal root unavailable")),
        })?;
        let snapshot =
            catalog_oplogs(root, &days).map_err(|error| catalog_error(journal_root, error))?;
        for (entry, file) in snapshot.into_catalogued_entries() {
            if !matches_raw_source(&entry, &self.source_slug)
                || self
                    .tracked
                    .iter()
                    .any(|tracked| tracked.entry.identity() == entry.identity())
            {
                continue;
            }
            let mut tracked = RawTrackedOplog {
                committed_frontier: entry.payload_offset() as u64,
                entry,
                file,
            };
            if let Some(bytes) = read_appended_bytes(&mut tracked)? {
                appended.push(bytes);
            }
            self.tracked.push(tracked);
        }
        Ok(appended)
    }
}

fn matches_raw_source(entry: &OplogCatalogEntry, source_slug: &str) -> bool {
    entry.name().source().display_slug() == source_slug && entry.name().format() == OplogFormat::Log
}

fn raw_follow_days(today: NaiveDate) -> Vec<NaiveDate> {
    today
        .pred_opt()
        .map_or_else(|| vec![today], |previous| vec![previous, today])
}

fn read_appended_bytes(tracked: &mut RawTrackedOplog) -> Result<Option<Vec<u8>>, FollowFatalError> {
    let frontier = tracked
        .file
        .metadata()
        .map_err(|error| raw_catalog_error(&tracked.entry, error))?
        .len();
    if frontier < tracked.committed_frontier {
        return Err(raw_catalog_identity_error(&tracked.entry));
    }
    let byte_count = usize::try_from(frontier - tracked.committed_frontier)
        .map_err(|_| raw_catalog_identity_error(&tracked.entry))?;
    if byte_count == 0 {
        return Ok(None);
    }
    let mut bytes = vec![0; byte_count];
    tracked
        .file
        .seek(SeekFrom::Start(tracked.committed_frontier))
        .and_then(|_| tracked.file.read_exact(&mut bytes))
        .map_err(|error| raw_catalog_error(&tracked.entry, error))?;
    tracked.committed_frontier = frontier;
    Ok(Some(bytes))
}

fn raw_catalog_error(entry: &OplogCatalogEntry, error: io::Error) -> FollowFatalError {
    FollowFatalError {
        path: PathBuf::from(entry.leaf()),
        operation: "catalog",
        source: Some(error),
    }
}

fn raw_catalog_identity_error(entry: &OplogCatalogEntry) -> FollowFatalError {
    FollowFatalError {
        path: PathBuf::from(entry.leaf()),
        operation: "catalog",
        source: Some(io::Error::other("oplog descriptor changed while following")),
    }
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
    let follower = OplogFollower::from_state(initial.state);
    run_source_follower_with_sleep(
        follower,
        journal_root,
        &source,
        &clock,
        stop,
        output,
        is_tty,
        sleep,
    )
}

fn run_source_follower_with_sleep(
    mut follower: OplogFollower,
    journal_root: &Path,
    source: &dyn OplogSnapshotSource,
    clock: &dyn OplogClock,
    stop: &dyn Fn() -> bool,
    output: &mut dyn Write,
    is_tty: bool,
    sleep: &dyn Fn(std::time::Duration),
) -> Result<(), FollowFatalError> {
    let mut last_service = None;
    while !stop() {
        let tick = follower
            .tick(source, clock, stop)
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

    use chrono::TimeZone;
    use solstone_core_journal_io::operational_log::{OplogFormat, create_oplog};

    use crate::collect::collect_source_tail_snapshot;

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

    #[test]
    fn snapshot_handoff_emits_only_bytes_appended_after_the_frontier() {
        struct StopAfterWrite<'a> {
            bytes: Vec<u8>,
            stop: &'a Cell<bool>,
        }

        impl Write for StopAfterWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                self.stop.set(true);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let opened = Local::now().fixed_offset();
        let mut writer = solstone_core_journal_io::operational_log::create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            "service",
            "supervisor",
            OplogFormat::Log,
            opened,
        )
        .unwrap();
        writeln!(writer, "before").unwrap();
        let snapshot =
            collect_source_tail_snapshot(temporary.path(), opened.naive_local(), "service", 1024)
                .unwrap();
        assert_eq!(snapshot.tail(), b"before\n");
        writeln!(writer, "after").unwrap();

        let stop = Cell::new(false);
        let mut output = StopAfterWrite {
            bytes: Vec::new(),
            stop: &stop,
        };
        run_follow_from_snapshot_with_sleep(
            snapshot,
            temporary.path(),
            &|| stop.get(),
            &mut output,
            false,
            &|_| {},
            &|| opened.date_naive(),
        )
        .unwrap();
        assert_eq!(output.bytes, b"after\n");
    }

    #[test]
    fn raw_snapshot_handoff_forwards_unterminated_lossy_large_bytes_once() {
        struct StopAfterWrite<'a> {
            bytes: Vec<u8>,
            stop: &'a Cell<bool>,
        }

        impl Write for StopAfterWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                self.stop.set(true);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let opened = Local::now().fixed_offset();
        let mut writer = solstone_core_journal_io::operational_log::create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            "service",
            "supervisor",
            OplogFormat::Log,
            opened,
        )
        .unwrap();
        writer.write_all(b"before\xff").unwrap();
        let snapshot =
            collect_source_tail_snapshot(temporary.path(), opened.naive_local(), "service", 1024)
                .unwrap();
        assert_eq!(snapshot.tail(), b"before\xff");

        let mut after = b"progress:\xff".to_vec();
        after.extend(std::iter::repeat_n(b'x', 16 * 1024 + 1));
        writer.write_all(&after).unwrap();
        writer.flush().unwrap();

        let stop = Cell::new(false);
        let mut output = StopAfterWrite {
            bytes: Vec::new(),
            stop: &stop,
        };
        run_follow_from_snapshot_with_sleep(
            snapshot,
            temporary.path(),
            &|| stop.get(),
            &mut output,
            false,
            &|_| {},
            &|| opened.date_naive(),
        )
        .unwrap();

        assert_eq!(
            output.bytes,
            String::from_utf8_lossy(&after).as_bytes(),
            "the follow handoff must render every appended raw byte once"
        );
    }

    #[test]
    fn raw_follower_discovers_the_next_local_day_without_replaying_the_snapshot() {
        struct StopAfterWrite<'a> {
            bytes: Vec<u8>,
            stop: &'a Cell<bool>,
        }

        impl Write for StopAfterWrite<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                self.stop.set(true);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let opened = chrono::FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 23, 59, 59)
            .single()
            .unwrap();
        let mut before = solstone_core_journal_io::operational_log::create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            "service",
            "supervisor",
            OplogFormat::Log,
            opened,
        )
        .unwrap();
        before.write_all(b"before midnight\n").unwrap();
        let snapshot =
            collect_source_tail_snapshot(temporary.path(), opened.naive_local(), "service", 1024)
                .unwrap();

        let next_day = opened.date_naive().succ_opt().unwrap();
        let next_opened = opened
            .offset()
            .from_local_datetime(&next_day.and_hms_opt(0, 0, 1).unwrap())
            .single()
            .unwrap();
        let mut after = solstone_core_journal_io::operational_log::create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            "service",
            "supervisor",
            OplogFormat::Log,
            next_opened,
        )
        .unwrap();
        after.write_all(b"after midnight\n").unwrap();
        after.flush().unwrap();

        let stop = Cell::new(false);
        let mut output = StopAfterWrite {
            bytes: Vec::new(),
            stop: &stop,
        };
        run_follow_from_snapshot_with_sleep(
            snapshot,
            temporary.path(),
            &|| stop.get(),
            &mut output,
            false,
            &|_| {},
            &|| next_day,
        )
        .unwrap();
        assert_eq!(output.bytes, b"after midnight\n");
    }
}
