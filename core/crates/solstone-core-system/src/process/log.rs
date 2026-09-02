// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{self, Write};
use std::path::PathBuf;

use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};

const CHRONICLE_DIR: &str = "chronicle";

/// Per-day operational-log writer bound to one canonical `oplog--` leaf.
pub struct DailyLogWriter {
    journal_root: PathBuf,
    reference: String,
    name: String,
    pinned: bool,
    current_day: String,
    writer: OplogWriter,
}

impl DailyLogWriter {
    /// Open an operational writer rooted under `chronicle/<day>/health/`.
    ///
    /// A supplied day pins historical/batch work to that day and disables
    /// midnight rollover; `None` follows the local day and rolls at midnight.
    pub fn new(
        journal_root: impl Into<PathBuf>,
        reference: impl Into<String>,
        name: impl Into<String>,
        day: Option<String>,
    ) -> io::Result<Self> {
        let journal_root = journal_root.into();
        let reference = reference.into();
        let name = name.into();
        validate_component("reference", &reference)?;
        validate_component("name", &name)?;

        let (pinned, current_day, instant) = match day {
            Some(day) => {
                validate_day(&day)?;
                let instant = noon_in_local_fixed_offset(&day)?;
                (true, day, instant)
            }
            None => {
                let instant = Local::now().fixed_offset();
                (false, instant.format("%Y%m%d").to_string(), instant)
            }
        };
        let writer = open_writer(&journal_root, &name, &reference, instant)?;
        Ok(Self {
            journal_root,
            reference,
            name,
            pinned,
            current_day,
            writer,
        })
    }

    /// The canonical path recorded by this writer's admitted leaf.
    pub fn path(&self) -> PathBuf {
        self.journal_root
            .join(CHRONICLE_DIR)
            .join(&self.current_day)
            .join("health")
            .join(self.writer.leaf_name())
    }

    /// Keep drain threads alive: rollover and write I/O errors are best effort.
    pub fn write(&mut self, message: &str) {
        if !self.pinned {
            let instant = Local::now().fixed_offset();
            let day_now = instant.format("%Y%m%d").to_string();
            if day_now != self.current_day {
                // Open before closing: a failure leaves the old lease and writer
                // usable, so the next drain retries the rollover.
                if let Ok(new_writer) =
                    open_writer(&self.journal_root, &self.name, &self.reference, instant)
                {
                    let old_writer = std::mem::replace(&mut self.writer, new_writer);
                    self.current_day = day_now;
                    drop(old_writer);
                }
            }
        }
        // Output drains intentionally swallow disk-full/write failures.
        let _ = self.writer.write_all(message.as_bytes());
        let _ = self.writer.flush();
    }
}

fn open_writer(
    root: &std::path::Path,
    source: &str,
    run: &str,
    instant: DateTime<FixedOffset>,
) -> io::Result<OplogWriter> {
    let root = JournalRoot::open(root).map_err(oplog_io)?;
    create_oplog_at(root, source, run, OplogFormat::Log, instant).map_err(oplog_io)
}

fn oplog_io(error: impl std::error::Error) -> io::Error {
    io::Error::other(error.to_string())
}

fn noon_in_local_fixed_offset(day: &str) -> io::Result<DateTime<FixedOffset>> {
    let day = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| invalid_day())?;
    let offset = Local::now().fixed_offset().offset().to_owned();
    offset
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).expect("valid noon"))
        .single()
        .ok_or_else(invalid_day)
}

fn validate_component(kind: &str, value: &str) -> io::Result<()> {
    if value.is_empty() || value.contains('/') || value.contains('\\') || value == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid operational log {kind}: path separators and '..' are not allowed"),
        ));
    }
    Ok(())
}

fn validate_day(day: &str) -> io::Result<()> {
    validate_component("day", day)?;
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map(|_| ())
        .map_err(|_| invalid_day())
}

fn invalid_day() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid operational log day: expected YYYYMMDD",
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn rejects_path_components_before_opening_operational_logs() {
        for (reference, name, day) in [
            ("../reference", "process", None),
            ("reference", "../process", None),
            ("reference", "process", Some("../day")),
            ("reference", "process", Some("not-a-day")),
        ] {
            let root = tempfile::tempdir().unwrap();
            let error =
                match DailyLogWriter::new(root.path(), reference, name, day.map(str::to_owned)) {
                    Ok(_) => panic!("unsafe component must be rejected"),
                    Err(error) => error,
                };
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(!root.path().join(CHRONICLE_DIR).exists());
        }
    }

    #[test]
    fn ac19_rollover_open_failure_retains_old_handle_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut writer =
            DailyLogWriter::new(root.path(), "ref", "process", Some("19990101".to_owned()))
                .expect("old-day writer");
        writer.pinned = false;
        let today = Local::now().format("%Y%m%d").to_string();
        fs::write(
            root.path().join(CHRONICLE_DIR).join(&today),
            "not a directory",
        )
        .expect("block new day directory");
        writer.write("old handle remains usable\n");
        assert_eq!(writer.current_day, "19990101");
        assert!(
            fs::read_to_string(writer.path())
                .expect("old log")
                .contains("old handle")
        );
    }

    #[test]
    fn same_source_writers_and_restart_are_distinct_canonical_leaves() {
        let root = tempfile::tempdir().unwrap();
        let mut first =
            DailyLogWriter::new(root.path(), "run", "source", Some("20260807".to_owned()))
                .expect("first writer");
        let mut second =
            DailyLogWriter::new(root.path(), "run", "source", Some("20260807".to_owned()))
                .expect("second writer");
        first.write("first\n");
        second.write("second\n");
        drop(first);
        drop(second);
        let mut restarted =
            DailyLogWriter::new(root.path(), "run", "source", Some("20260807".to_owned()))
                .expect("restarted writer");
        restarted.write("restart\n");
        let health = root.path().join("chronicle/20260807/health");
        let leaves = fs::read_dir(&health)
            .expect("health")
            .map(|entry| entry.expect("entry"))
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("oplog--"))
            .collect::<Vec<_>>();
        assert_eq!(leaves.len(), 3);
        assert!(leaves.iter().all(|entry| {
            entry.path().is_file()
                && !fs::symlink_metadata(entry.path())
                    .expect("metadata")
                    .file_type()
                    .is_symlink()
        }));
    }

    #[test]
    fn historical_writer_stays_in_its_requested_partition() {
        let root = tempfile::tempdir().unwrap();
        let mut writer =
            DailyLogWriter::new(root.path(), "run", "source", Some("19990101".to_owned()))
                .expect("pinned writer");
        writer.write("historical\n");
        assert!(
            writer
                .path()
                .starts_with(root.path().join("chronicle/19990101/health"))
        );
        assert!(
            !root
                .path()
                .join("chronicle")
                .join(Local::now().format("%Y%m%d").to_string())
                .join("health")
                .exists()
        );
    }
}
