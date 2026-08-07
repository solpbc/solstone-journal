// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{get_journal_config_path, materialized_defaults};

/// The result of one strict, single-buffer journal configuration read.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalConfigRead {
    /// Whether `config/journal.json` was present when read.
    pub present: bool,
    /// SHA-256 of the exact bytes read, when the config was present and valid.
    pub sha256: Option<String>,
    /// Parsed configuration object, when the config was present and valid.
    pub config: Option<Map<String, Value>>,
}

/// The configuration map a mutation starts from.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalConfigMutationBase {
    /// Existing strict-read config or freshly materialized defaults.
    pub config: Map<String, Value>,
    /// Whether `config` was materialized because the file was missing.
    pub materialized: bool,
}

/// Strict-load failure for an existing `config/journal.json`.
///
/// This intentionally carries no decoded map, mutation value, or transaction;
/// callers cannot obtain a writable configuration from a failed load.
///
/// ```compile_fail,E0026
/// use solstone_core_journal_config::ConfigLoadError;
///
/// fn extract_writable_config(error: ConfigLoadError) {
///     let ConfigLoadError::Corrupt { config, .. } = error;
///     drop(config);
/// }
/// ```
#[derive(Debug)]
pub enum ConfigLoadError {
    /// The existing file could not be read, parsed, or interpreted as an object.
    Corrupt {
        /// Existing configuration path.
        path: PathBuf,
        /// Low-level cause retained only for diagnostics.
        source: Box<dyn Error + Send + Sync>,
    },
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt { path, .. } => write!(
                formatter,
                "I couldn't read your settings file at {}. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.",
                path.display()
            ),
        }
    }
}

impl Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Corrupt { source, .. } => Some(source.as_ref()),
        }
    }
}

/// Read a journal configuration from exactly one filesystem byte buffer.
pub fn read_journal_config(journal_path: &Path) -> Result<JournalConfigRead, ConfigLoadError> {
    read_config_path(&get_journal_config_path(journal_path))
}

/// Load the single-read config basis used by the journal-I/O mutation owner.
pub fn load_mutation_base(
    journal_path: &Path,
) -> Result<JournalConfigMutationBase, ConfigLoadError> {
    let read = read_journal_config(journal_path)?;
    match read.config {
        Some(config) => Ok(JournalConfigMutationBase {
            config,
            materialized: false,
        }),
        None => Ok(JournalConfigMutationBase {
            config: materialized_defaults(),
            materialized: true,
        }),
    }
}

fn read_config_path(path: &Path) -> Result<JournalConfigRead, ConfigLoadError> {
    let bytes = match read_bytes(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok(JournalConfigRead {
                present: false,
                sha256: None,
                config: None,
            });
        }
        Err(source) => return Err(corrupt(path, source)),
    };
    let sha256 = format!("sha256:{:x}", Sha256::digest(&bytes));
    let value = serde_json::from_slice::<Value>(&bytes).map_err(|source| corrupt(path, source))?;
    let config = value.as_object().cloned().ok_or_else(|| {
        corrupt(
            path,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "journal config must be a JSON object",
            ),
        )
    })?;
    Ok(JournalConfigRead {
        present: true,
        sha256: Some(sha256),
        config: Some(config),
    })
}

fn corrupt(path: &Path, source: impl Error + Send + Sync + 'static) -> ConfigLoadError {
    ConfigLoadError::Corrupt {
        path: path.to_path_buf(),
        source: Box::new(source),
    }
}

pub(crate) trait ReadSource {
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, io::Error>;
}

struct FilesystemReadSource;

impl ReadSource for FilesystemReadSource {
    fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, io::Error> {
        fs::read(path)
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, io::Error> {
    #[cfg(test)]
    if let Some(source) = test_read_source() {
        return source.read_bytes(path);
    }
    FilesystemReadSource.read_bytes(path)
}

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::rc::Rc;

#[cfg(test)]
thread_local! {
    static TEST_READ_SOURCE: RefCell<Option<Rc<dyn ReadSource>>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn test_read_source() -> Option<Rc<dyn ReadSource>> {
    TEST_READ_SOURCE.with(|source| source.borrow().clone())
}

#[cfg(test)]
pub(crate) struct ReadSourceGuard {
    previous: Option<Rc<dyn ReadSource>>,
}

#[cfg(test)]
impl Drop for ReadSourceGuard {
    fn drop(&mut self) {
        TEST_READ_SOURCE.with(|source| {
            *source.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(test)]
pub(crate) fn install_read_source(source: Rc<dyn ReadSource>) -> ReadSourceGuard {
    let previous = TEST_READ_SOURCE.with(|current| current.replace(Some(source)));
    ReadSourceGuard { previous }
}
