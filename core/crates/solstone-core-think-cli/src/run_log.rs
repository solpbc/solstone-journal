// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::args::ThinkArgs;

pub(crate) fn mode(args: &ThinkArgs) -> &'static str {
    if args.activity.is_some() {
        "activity"
    } else if args.flush {
        "flush"
    } else if args.segments || args.segment.is_some() {
        "segment"
    } else if args.weekly {
        "weekly"
    } else if args.cadence {
        "cadence"
    } else {
        "daily"
    }
}

pub(crate) fn path(day: &Path, now_ms: i64, mode: &str) -> PathBuf {
    day.join("health").join(format!("{now_ms}_{mode}.jsonl"))
}

static NEXT_COLLISION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_path(log_dir: &Path, started_at_ms: i64, pid: u32, sequence: u64, mode: &str) -> PathBuf {
    log_dir.join(format!("{started_at_ms}_{pid}_{sequence}_{mode}.jsonl"))
}

#[derive(Debug)]
pub(crate) enum RunLogError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Poisoned,
    Retained { path: PathBuf, detail: String },
}

impl fmt::Display for RunLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Poisoned => formatter.write_str("run log writer lock is poisoned"),
            Self::Retained { path, detail } => {
                write!(
                    formatter,
                    "think run log {} failed: {detail}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for RunLogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Poisoned | Self::Retained { .. } => None,
        }
    }
}

impl From<std::io::Error> for RunLogError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RunLogError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub(crate) struct RunLogWriter<W: Write> {
    sink: Option<Mutex<W>>,
    display_path: PathBuf,
    skip_count: AtomicUsize,
    first_error: Mutex<Option<String>>,
}

impl RunLogWriter<std::fs::File> {
    pub(crate) fn open(path: &Path) -> Self {
        let display_path = path.to_path_buf();
        let (sink, first_error) = match path.parent().map(std::fs::create_dir_all).transpose() {
            Ok(_) => match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => (Some(file), None),
                Err(error) => (None, Some(error.to_string())),
            },
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            sink: sink.map(Mutex::new),
            display_path,
            skip_count: AtomicUsize::new(0),
            first_error: Mutex::new(first_error),
        }
    }

    pub(crate) fn create_unique(
        log_dir: &Path,
        started_at_ms: i64,
        mode: &str,
    ) -> Result<Self, RunLogError> {
        std::fs::create_dir_all(log_dir)?;
        let pid = std::process::id();
        let mut sequence = 0;
        loop {
            let display_path = unique_path(log_dir, started_at_ms, pid, sequence, mode);
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&display_path)
            {
                Ok(file) => {
                    return Ok(Self {
                        sink: Some(Mutex::new(file)),
                        display_path,
                        skip_count: AtomicUsize::new(0),
                        first_error: Mutex::new(None),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    sequence = NEXT_COLLISION_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub(crate) fn create_unique_retaining_failure(
        log_dir: &Path,
        started_at_ms: i64,
        mode: &str,
    ) -> Self {
        match Self::create_unique(log_dir, started_at_ms, mode) {
            Ok(writer) => writer,
            Err(error) => Self {
                sink: None,
                display_path: unique_path(log_dir, started_at_ms, std::process::id(), 0, mode),
                skip_count: AtomicUsize::new(0),
                first_error: Mutex::new(Some(error.to_string())),
            },
        }
    }
}

impl<W: Write> RunLogWriter<W> {
    #[cfg(test)]
    pub(crate) fn with_sink(display_path: PathBuf, sink: W) -> Self {
        Self {
            sink: Some(Mutex::new(sink)),
            display_path,
            skip_count: AtomicUsize::new(0),
            first_error: Mutex::new(None),
        }
    }

    pub(crate) fn log<T: Serialize>(&self, record: &T) -> Result<(), RunLogError> {
        let mut encoded = serde_json::to_vec(record)?;
        encoded.push(b'\n');
        let Some(sink) = self.sink.as_ref() else {
            return Ok(());
        };
        let mut sink = sink.lock().map_err(|_| RunLogError::Poisoned)?;
        sink.write_all(&encoded)?;
        sink.flush()?;
        Ok(())
    }

    pub(crate) fn log_event(&self, event: &str, now_ms: i64, fields: Map<String, Value>) {
        if event == "talent.skip" {
            self.skip_count.fetch_add(1, Ordering::Relaxed);
        }
        let mut data = fields;
        data.insert("event".to_owned(), Value::String(event.to_owned()));
        data.insert("ts".to_owned(), Value::from(now_ms));
        if let Err(error) = self.log(&data) {
            self.remember_error(error.to_string());
        }
    }

    fn remember_error(&self, detail: String) {
        let mut first_error = self
            .first_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if first_error.is_none() {
            *first_error = Some(detail);
        }
    }

    pub(crate) fn finish(&self) -> Result<(), RunLogError> {
        let first_error = self.first_error.lock().map_err(|_| RunLogError::Poisoned)?;
        match first_error.as_ref() {
            Some(detail) => Err(RunLogError::Retained {
                path: self.display_path.clone(),
                detail: detail.clone(),
            }),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn skip_count(&self) -> usize {
        self.skip_count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::{Map, Value, json};
    use tempfile::tempdir;

    use super::{RunLogWriter, unique_path};

    struct PartialThenError(bool);

    impl Write for PartialThenError {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 {
                Err(io::Error::other("partial write failed"))
            } else {
                self.0 = true;
                Ok(bytes.len().min(1))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct WriteZero;

    impl Write for WriteZero {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Ok(0)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FlushFailure;

    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[test]
    fn create_unique_retries_with_a_distinct_sequence() {
        let temporary = tempdir().unwrap();
        let first = RunLogWriter::create_unique(temporary.path(), 42, "segments").unwrap();
        let second = RunLogWriter::create_unique(temporary.path(), 42, "segments").unwrap();

        first.log(&json!({"writer":"first"})).unwrap();
        second.log(&json!({"writer":"second"})).unwrap();

        assert_eq!(
            first.display_path,
            unique_path(temporary.path(), 42, std::process::id(), 0, "segments")
        );
        assert_eq!(
            second.display_path,
            unique_path(temporary.path(), 42, std::process::id(), 1, "segments")
        );
        assert_eq!(
            fs::read_to_string(&first.display_path).unwrap(),
            "{\"writer\":\"first\"}\n"
        );
        assert_eq!(
            fs::read_to_string(&second.display_path).unwrap(),
            "{\"writer\":\"second\"}\n"
        );
    }

    #[test]
    fn unique_path_includes_process_id() {
        let root = Path::new("/run-logs");
        assert_ne!(
            unique_path(root, 42, 100, 0, "segments"),
            unique_path(root, 42, 101, 0, "segments")
        );
    }

    #[test]
    fn shared_writer_keeps_concurrent_records_complete() {
        const THREADS: usize = 8;
        const RECORDS_PER_THREAD: usize = 100;

        let temporary = tempdir().unwrap();
        let writer =
            Arc::new(RunLogWriter::create_unique(temporary.path(), 42, "segments").unwrap());
        std::thread::scope(|scope| {
            for thread in 0..THREADS {
                let writer = Arc::clone(&writer);
                scope.spawn(move || {
                    for record in 0..RECORDS_PER_THREAD {
                        writer
                            .log(&json!({"thread":thread,"record":record}))
                            .unwrap();
                    }
                });
            }
        });

        let records = fs::read_to_string(&writer.display_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), THREADS * RECORDS_PER_THREAD);
        assert!(
            records
                .iter()
                .all(|record| { record.get("thread").is_some() && record.get("record").is_some() })
        );
    }

    #[test]
    fn partial_write_write_zero_and_flush_failures_are_retained() {
        let cases: Vec<(Box<dyn Write + Send>, &str)> = vec![
            (Box::new(PartialThenError(false)), "partial write failed"),
            (Box::new(WriteZero), "failed to write whole buffer"),
            (Box::new(FlushFailure), "flush failed"),
        ];
        for (sink, expected) in cases {
            let writer = RunLogWriter::with_sink(PathBuf::from("run.jsonl"), sink);
            writer.log_event("run.start", 1, Map::new());
            assert!(
                writer.finish().unwrap_err().to_string().contains(expected),
                "expected {expected}"
            );
        }
    }
}
