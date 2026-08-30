// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
#[cfg(unix)]
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use solstone_core_journal_io::{JournalRoot, open_flat_directory_bound, read_bytes_bound};

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

/// Read a journal configuration through an already-admitted journal root.
///
/// The retained root descriptor is the source authority. Its canonical path is
/// used only to preserve diagnostic paths in [`ConfigLoadError`].
#[cfg(unix)]
pub fn read_journal_config_bound(root: &JournalRoot) -> Result<JournalConfigRead, ConfigLoadError> {
    let config_path = root.canonical_path().join("config/journal.json");
    root.revalidate()
        .map_err(|source| corrupt(&config_path, source))?;
    let Some(config_directory) =
        open_flat_directory_bound(root, OsStr::new("config"), root.canonical_path())
            .map_err(|source| corrupt(&config_path, source))?
    else {
        return parse_config_bytes(&config_path, None);
    };
    let bytes = read_bytes_bound(&config_directory, OsStr::new("journal.json"))
        .map_err(|source| corrupt(&config_path, source))?;
    parse_config_bytes(&config_path, bytes)
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
        Ok(bytes) => Some(bytes),
        Err(source) if source.kind() == io::ErrorKind::NotFound => None,
        Err(source) => return Err(corrupt(path, source)),
    };
    parse_config_bytes(path, bytes)
}

fn parse_config_bytes(
    path: &Path,
    bytes: Option<Vec<u8>>,
) -> Result<JournalConfigRead, ConfigLoadError> {
    let Some(bytes) = bytes else {
        return Ok(JournalConfigRead {
            present: false,
            sha256: None,
            config: None,
        });
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

#[cfg(all(test, unix))]
mod bound_tests {
    use solstone_core_journal_io::{
        BoundReadPrimitive, JournalRoot, run_with_bound_read_barrier,
        run_with_two_bound_read_barriers,
    };
    use std::fs;
    use std::os::unix::net::UnixListener;

    use super::{ConfigLoadError, read_journal_config, read_journal_config_bound};
    use crate::test_support::TempDir;

    fn write_config(root: &std::path::Path, bytes: &[u8]) {
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("journal.json"), bytes).unwrap();
    }

    fn assert_path_and_bound_match(root: &std::path::Path) {
        let path_result = read_journal_config(root);
        let admitted = JournalRoot::open(root).unwrap();
        let bound_result = read_journal_config_bound(&admitted);
        match (path_result, bound_result) {
            (Ok(path_read), Ok(bound_read)) => assert_eq!(bound_read, path_read),
            (Err(ConfigLoadError::Corrupt { .. }), Err(ConfigLoadError::Corrupt { .. })) => {}
            (path, bound) => {
                panic!("path and bound reads diverged: path={path:?}, bound={bound:?}")
            }
        }
    }

    #[test]
    fn bound_reader_matches_path_reader_for_config_states() {
        let missing_directory = TempDir::new();
        assert_path_and_bound_match(missing_directory.path());

        let missing_file = TempDir::new();
        fs::create_dir(missing_file.path().join("config")).unwrap();
        assert_path_and_bound_match(missing_file.path());

        let enabled = TempDir::new();
        write_config(enabled.path(), br#"{"mcp_endpoint":{"enabled":true}}"#);
        assert_path_and_bound_match(enabled.path());

        let absent = TempDir::new();
        write_config(absent.path(), br#"{"identity":{"name":"Ada"}}"#);
        assert_path_and_bound_match(absent.path());

        let malformed = TempDir::new();
        write_config(malformed.path(), b"{");
        assert_path_and_bound_match(malformed.path());
    }

    #[test]
    fn bound_reader_keeps_the_admitted_root_after_a_path_swap() {
        let temporary = TempDir::new();
        let original = temporary.path().join("original");
        let replacement = temporary.path().join("replacement");
        let moved = temporary.path().join("moved");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&replacement).unwrap();
        write_config(&original, br#"{"origin":"original"}"#);
        write_config(&replacement, br#"{"origin":"replacement"}"#);

        let admitted = JournalRoot::open(&original).unwrap();
        fs::rename(&original, &moved).unwrap();
        fs::rename(&replacement, &original).unwrap();

        let bound = read_journal_config_bound(&admitted).unwrap();
        let path = read_journal_config(&original).unwrap();
        assert_eq!(bound.config.as_ref().unwrap()["origin"], "original");
        assert_eq!(path.config.as_ref().unwrap()["origin"], "replacement");
    }

    #[test]
    fn bound_reader_rejects_config_substitution_and_removal_races() {
        let socket = TempDir::new();
        write_config(socket.path(), br#"{"origin":"original"}"#);
        let socket_path = socket.path().join("config/journal.json");
        let admitted = JournalRoot::open(socket.path()).unwrap();
        let (result, fired) = run_with_bound_read_barrier(
            BoundReadPrimitive::Open,
            1,
            move || {
                fs::remove_file(&socket_path).unwrap();
                let listener = UnixListener::bind(&socket_path).unwrap();
                drop(listener);
            },
            || read_journal_config_bound(&admitted),
        );
        assert!(fired);
        assert!(matches!(result, Err(ConfigLoadError::Corrupt { .. })));

        let removed = TempDir::new();
        write_config(removed.path(), br#"{"origin":"original"}"#);
        let removed_path = removed.path().join("config/journal.json");
        let admitted = JournalRoot::open(removed.path()).unwrap();
        let (result, fired) = run_with_bound_read_barrier(
            BoundReadPrimitive::Read,
            1,
            move || fs::remove_file(&removed_path).unwrap(),
            || read_journal_config_bound(&admitted),
        );
        assert!(fired);
        assert!(matches!(result, Err(ConfigLoadError::Corrupt { .. })));
    }

    #[test]
    fn bound_reader_rejects_config_regular_file_replacement_after_open() {
        let temporary = TempDir::new();
        write_config(temporary.path(), br#"{"origin":"original"}"#);
        let target = temporary.path().join("config/journal.json");
        let aside = temporary.path().join("config/journal.json.aside");
        let replacement = temporary.path().join("config/replacement.json");
        fs::write(&replacement, br#"{"origin":"replacement"}"#).unwrap();
        let admitted = JournalRoot::open(temporary.path()).unwrap();
        let (result, fired) = run_with_two_bound_read_barriers(
            BoundReadPrimitive::Read,
            1,
            move || {
                fs::rename(&target, &aside).unwrap();
                fs::rename(&replacement, &target).unwrap();
            },
            BoundReadPrimitive::FinalNameObserve,
            1,
            || {},
            || read_journal_config_bound(&admitted),
        );
        assert_eq!(fired, 2);
        assert!(matches!(result, Err(ConfigLoadError::Corrupt { .. })));
    }
}
