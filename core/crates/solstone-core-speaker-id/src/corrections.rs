// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read and append `speaker_corrections.json` payloads.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, atomic_replace, hold_lock,
};

use crate::json::{JsonError, write_python_compatible_json};

const CORRECTIONS_FILE: &str = "speaker_corrections.json";

/// Errors produced while reading or appending speaker corrections.
#[derive(Debug)]
pub enum CorrectionsError {
    Lock(LockError),
    Read { path: PathBuf, source: io::Error },
    Malformed { path: PathBuf },
    Serialize(JsonError),
    Write(AtomicWriteError),
}

impl fmt::Display for CorrectionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => write!(f, "could not lock speaker corrections: {error}"),
            Self::Read { path, source } => {
                write!(
                    f,
                    "could not read speaker corrections at {}: {source}",
                    path.display()
                )
            }
            Self::Malformed { path } => {
                write!(f, "speaker corrections at {} are malformed", path.display())
            }
            Self::Serialize(error) => write!(f, "could not serialize speaker corrections: {error}"),
            Self::Write(error) => write!(f, "could not write speaker corrections: {error}"),
        }
    }
}

impl Error for CorrectionsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::Malformed { .. } => None,
            Self::Serialize(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// Append one correction to a segment's correction log.
pub fn append_correction(
    segment_dir: &Path,
    correction: Map<String, Value>,
) -> Result<(), CorrectionsError> {
    let path = corrections_path(segment_dir);
    let _lock = hold_lock(&path, LockOptions::default()).map_err(CorrectionsError::Lock)?;

    let mut corrections = read_corrections_path(&path)?;
    corrections.push(Value::Object(correction));

    let mut payload = Map::new();
    payload.insert("corrections".to_owned(), Value::Array(corrections));
    let bytes = write_python_compatible_json(&Value::Object(payload), 2)
        .map_err(CorrectionsError::Serialize)?
        .into_bytes();

    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(CorrectionsError::Write)
}

/// Read a segment's corrections, distinguishing an absent file from malformed data.
pub fn read_corrections(segment_dir: &Path) -> Result<Vec<Value>, CorrectionsError> {
    read_corrections_path(&corrections_path(segment_dir))
}

fn corrections_path(segment_dir: &Path) -> PathBuf {
    segment_dir.join("talents").join(CORRECTIONS_FILE)
}

fn read_corrections_path(path: &Path) -> Result<Vec<Value>, CorrectionsError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(CorrectionsError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };

    let value: Value = serde_json::from_slice(&bytes).map_err(|_| CorrectionsError::Malformed {
        path: path.to_owned(),
    })?;
    let object = value
        .as_object()
        .ok_or_else(|| CorrectionsError::Malformed {
            path: path.to_owned(),
        })?;

    match object.get("corrections") {
        None => Ok(Vec::new()),
        Some(Value::Array(corrections)) => Ok(corrections.clone()),
        Some(_) => Err(CorrectionsError::Malformed {
            path: path.to_owned(),
        }),
    }
}
