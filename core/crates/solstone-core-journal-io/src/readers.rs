// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Tolerant readers with an explicit malformed-data posture.

use std::fs;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::errors::{MalformedDataError, ReadError};

/// Response to malformed non-empty JSON data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedPolicy {
    /// Omit malformed data without emitting a log record.
    Skip,
    /// Omit malformed data and emit a warning.
    WarnAndSkip,
    /// Return a typed read error.
    Raise,
}

/// Read JSON, treating missing or empty files as `default` before malformed policy.
pub fn read_json<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    default: T,
    on_error: MalformedPolicy,
) -> Result<T, ReadError> {
    let path = path.as_ref();
    let contents = read_missing_as_empty(path)?;
    if contents.is_empty() {
        return Ok(default);
    }
    match serde_json::from_slice(&contents) {
        Ok(value) => Ok(value),
        Err(source) => malformed_or_default(path, None, source, default, on_error),
    }
}

/// Read JSONL, omitting blank lines and applying malformed policy per record.
pub fn read_jsonl<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    default: Vec<T>,
    on_error: MalformedPolicy,
) -> Result<Vec<T>, ReadError> {
    let path = path.as_ref();
    let contents = read_missing_as_empty(path)?;
    if contents.is_empty() {
        return Ok(default);
    }
    let mut records = Vec::new();
    for (index, line) in contents.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(record) => records.push(record),
            Err(source) => match on_error {
                MalformedPolicy::Raise => {
                    return Err(ReadError::Malformed(MalformedDataError {
                        path: path.to_path_buf(),
                        line: Some(index + 1),
                        source,
                    }));
                }
                MalformedPolicy::Skip => {}
                MalformedPolicy::WarnAndSkip => {
                    log::warn!(
                        "malformed JSONL record in {} at line {}: {source}",
                        path.display(),
                        index + 1
                    );
                }
            },
        }
    }
    Ok(records)
}

/// Read UTF-8 text, treating a missing path as `default`.
pub fn read_text(path: impl AsRef<Path>, default: String) -> Result<String, ReadError> {
    let path = path.as_ref();
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
        Err(source) => Err(io_error(path, source)),
    }
}

fn read_missing_as_empty(path: &Path) -> Result<Vec<u8>, ReadError> {
    match fs::read(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(io_error(path, source)),
    }
}

fn malformed_or_default<T>(
    path: &Path,
    line: Option<usize>,
    source: serde_json::Error,
    default: T,
    on_error: MalformedPolicy,
) -> Result<T, ReadError> {
    match on_error {
        MalformedPolicy::Raise => Err(ReadError::Malformed(MalformedDataError {
            path: path.to_path_buf(),
            line,
            source,
        })),
        MalformedPolicy::Skip => Ok(default),
        MalformedPolicy::WarnAndSkip => {
            log::warn!("malformed JSON data in {}: {source}", path.display());
            Ok(default)
        }
    }
}

fn io_error(path: &Path, source: io::Error) -> ReadError {
    ReadError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Mutex, Once, OnceLock};

    use log::{Level, LevelFilter, Log, Metadata, Record};

    use super::*;
    use crate::test_support::TempDir;

    struct TestLogger;

    static LOGGER: TestLogger = TestLogger;
    static LOGGER_INIT: Once = Once::new();
    static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

    impl Log for TestLogger {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.level() <= Level::Warn
        }

        fn log(&self, record: &Record<'_>) {
            if self.enabled(record.metadata()) {
                LOGS.get_or_init(|| Mutex::new(Vec::new()))
                    .lock()
                    .unwrap()
                    .push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    fn install_logger() {
        LOGGER_INIT.call_once(|| {
            log::set_logger(&LOGGER).unwrap();
            log::set_max_level(LevelFilter::Warn);
        });
        LOGS.get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clear();
    }

    #[test]
    fn missing_and_empty_json_return_defaults_before_policy() {
        let temporary = TempDir::new();
        let missing = temporary.path().join("missing.json");
        let empty = temporary.path().join("empty.json");
        fs::write(&empty, []).unwrap();

        assert_eq!(
            read_json::<Vec<u8>>(&missing, vec![1], MalformedPolicy::Raise).unwrap(),
            vec![1]
        );
        assert_eq!(
            read_json::<Vec<u8>>(&empty, vec![2], MalformedPolicy::Raise).unwrap(),
            vec![2]
        );
        assert_eq!(
            read_jsonl::<u8>(&missing, vec![3], MalformedPolicy::Raise).unwrap(),
            vec![3]
        );
        assert_eq!(
            read_jsonl::<u8>(&empty, vec![4], MalformedPolicy::Raise).unwrap(),
            vec![4]
        );
    }

    #[test]
    fn malformed_jsonl_policy_controls_omission_and_line_number() {
        install_logger();
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        fs::write(&path, b"1\nnot-json\n2\n").unwrap();

        let error = read_jsonl::<u8>(&path, Vec::new(), MalformedPolicy::Raise).unwrap_err();
        match error {
            ReadError::Malformed(error) => assert_eq!(error.line, Some(2)),
            ReadError::Io { .. } => panic!("expected malformed data"),
        }
        assert_eq!(
            read_jsonl::<u8>(&path, Vec::new(), MalformedPolicy::Skip).unwrap(),
            vec![1, 2]
        );
        assert_eq!(
            read_jsonl::<u8>(&path, Vec::new(), MalformedPolicy::WarnAndSkip).unwrap(),
            vec![1, 2]
        );
        assert!(
            LOGS.get()
                .unwrap()
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.contains("line 2"))
        );
    }
}
