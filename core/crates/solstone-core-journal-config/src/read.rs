// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
#[cfg(any(unix, windows))]
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::civil::Date;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
#[cfg(any(unix, windows))]
use solstone_core_journal_io::JournalRoot;
#[cfg(any(unix, windows))]
use solstone_core_journal_io::media::{MediaDirectory, MediaEntry};
#[cfg(unix)]
use solstone_core_journal_io::{open_flat_directory_bound, read_bytes_bound};
#[cfg(windows)]
use solstone_core_journal_io::{
    open_windows_flat_directory_bound, read_windows_observed_file_bounded,
};

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
                "your settings file at {} couldn't be read. your settings were not changed. repair the file or restore config/journal.json from a backup, then try again.",
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
#[cfg(any(unix, windows))]
pub fn read_journal_config_bound(root: &JournalRoot) -> Result<JournalConfigRead, ConfigLoadError> {
    let config_path = root.canonical_path().join("config/journal.json");
    root.revalidate()
        .map_err(|source| corrupt(&config_path, source))?;
    #[cfg(unix)]
    let config_directory =
        open_flat_directory_bound(root, OsStr::new("config"), root.canonical_path())
            .map_err(|source| corrupt(&config_path, source))?;
    #[cfg(windows)]
    let config_directory =
        open_windows_flat_directory_bound(root, OsStr::new("config"), root.canonical_path())
            .map_err(|source| corrupt(&config_path, source))?;
    let Some(config_directory) = config_directory else {
        return parse_config_bytes(&config_path, None);
    };
    #[cfg(unix)]
    let bytes = read_bytes_bound(&config_directory, OsStr::new("journal.json"))
        .map_err(|source| corrupt(&config_path, source))?;
    #[cfg(windows)]
    let bytes = read_windows_observed_file_bounded(
        &config_directory,
        OsStr::new("journal.json"),
        usize::MAX,
    )
    .map_err(|source| corrupt(&config_path, source))?
    .map(|observation| observation.bytes);
    parse_config_bytes(&config_path, bytes)
}

/// Strictly decode bounded, complete bytes from a retained configuration parent.
/// The directory's spelling is diagnostic only; Native checks its chain and
/// the final file name around the read. Missing files never materialize defaults.
#[cfg(any(unix, windows))]
pub fn read_journal_config_in_directory(
    directory: &MediaDirectory,
    max_bytes: usize,
) -> Result<JournalConfigRead, ConfigLoadError> {
    read_journal_config_observed_in_directory(directory, max_bytes).map(|(read, _)| read)
}

/// Decode and fingerprint the same complete byte buffer as its retained file observation.
#[cfg(any(unix, windows))]
pub fn read_journal_config_observed_in_directory(
    directory: &MediaDirectory,
    max_bytes: usize,
) -> Result<(JournalConfigRead, Option<MediaEntry>), ConfigLoadError> {
    let path = directory.diagnostic_path().join("journal.json");
    let read = directory
        .read_file_bounded(OsStr::new("journal.json"), max_bytes)
        .map_err(|source| corrupt(&path, source))?;
    let (bytes, entry) = match read {
        Some(read) if read.complete => (Some(read.bytes), Some(read.entry)),
        Some(_) => {
            return Err(corrupt(
                &path,
                io::Error::new(io::ErrorKind::InvalidData, "incomplete configuration read"),
            ));
        }
        None => (None, None),
    };
    parse_config_bytes(&path, bytes).map(|read| (read, entry))
}

/// A registered daily-input cutoff or its containing map is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyInputSinceError;

impl fmt::Display for DailyInputSinceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("daily_inputs must map input identities and versions to YYYYMMDD dates")
    }
}
impl Error for DailyInputSinceError {}

/// Parse exactly one `YYYYMMDD` date.
pub fn parse_daily_input_day(day: &str) -> Result<Date, DailyInputSinceError> {
    if day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DailyInputSinceError);
    }
    solstone_core_time::parse_day(day).ok_or(DailyInputSinceError)
}

/// Read an input/version's immutable activation cutoff without writing.
pub fn daily_input_since(
    config: &Map<String, Value>,
    id: &str,
    version: &str,
) -> Result<Option<Date>, DailyInputSinceError> {
    let Some(inputs) = config.get("daily_inputs") else {
        return Ok(None);
    };
    let inputs = inputs.as_object().ok_or(DailyInputSinceError)?;
    let Some(versions) = inputs.get(id) else {
        return Ok(None);
    };
    let versions = versions.as_object().ok_or(DailyInputSinceError)?;
    match versions.get(version) {
        None => Ok(None),
        Some(Value::String(day)) => parse_daily_input_day(day).map(Some),
        Some(_) => Err(DailyInputSinceError),
    }
}

/// Fingerprint supplied bytes using the journal configuration reader's encoding.
pub fn fingerprint_journal_config_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
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

/// Whether the journal has no valid thinking provider configured in `config/journal.json`.
pub fn no_thinking_engine_chosen(journal: &Path) -> bool {
    let Ok(read) = read_journal_config(journal) else {
        return true;
    };
    let Some(config) = read.config else {
        return true;
    };
    !config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active").and_then(Value::as_object))
        .and_then(|active| active.get("provider"))
        .and_then(Value::as_str)
        .is_some_and(|provider| !provider.trim().is_empty())
}

/// A thinking lane saved during setup before an engine is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SavedLane {
    Byo,
    Spp,
    Local,
}

impl SavedLane {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Byo => "byo",
            Self::Spp => "spp",
            Self::Local => "local",
        }
    }
}

/// Return the thinking lane saved during setup, if one was chosen and remains valid.
pub fn saved_thinking_lane(journal: &Path) -> Option<SavedLane> {
    let Ok(read) = read_journal_config(journal) else {
        return None;
    };
    let config = read.config?;
    let choice = config
        .get("setup")
        .and_then(Value::as_object)
        .and_then(|setup| setup.get("thinking_choice"))
        .and_then(Value::as_str)?;
    match choice {
        "byo" => Some(SavedLane::Byo),
        "spp" => Some(SavedLane::Spp),
        "local" => Some(SavedLane::Local),
        _ => None,
    }
}

/// Return whether tier-8 entity typo auto-acceptance is enabled.
pub fn entity_tier8_typo_acceptance_enabled(config: Option<&Map<String, Value>>) -> bool {
    let Some(config) = config else {
        return false;
    };
    config
        .get("entities")
        .and_then(Value::as_object)
        .and_then(|entities| entities.get("accept_tier8_typos"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
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
    let sha256 = fingerprint_journal_config_bytes(&bytes);
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
    fn bound_reader_rejects_config_removal_during_read() {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_support::TempDir;

    fn write_config(root: &std::path::Path, bytes: &[u8]) {
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("journal.json"), bytes).unwrap();
    }

    #[test]
    fn no_thinking_engine_chosen_detects_absent_empty_and_valid_providers() {
        let temp = TempDir::new();
        assert!(no_thinking_engine_chosen(temp.path()));

        write_config(temp.path(), br#"{"providers":{}}"#);
        assert!(no_thinking_engine_chosen(temp.path()));

        write_config(
            temp.path(),
            br#"{"providers":{"active":{"provider":"   "}}}"#,
        );
        assert!(no_thinking_engine_chosen(temp.path()));

        write_config(
            temp.path(),
            br#"{"providers":{"active":{"provider":"anthropic"}}}"#,
        );
        assert!(!no_thinking_engine_chosen(temp.path()));
    }

    #[test]
    fn saved_thinking_lane_detects_known_choices_and_ignores_others() {
        let temp = TempDir::new();
        // absent file
        assert_eq!(saved_thinking_lane(temp.path()), None);

        // unreadable bytes (invalid JSON)
        write_config(temp.path(), b"not json");
        assert_eq!(saved_thinking_lane(temp.path()), None);

        // missing setup
        write_config(temp.path(), br#"{"providers":{}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), None);

        // "later"
        write_config(temp.path(), br#"{"setup":{"thinking_choice":"later"}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), None);

        // other string
        write_config(temp.path(), br#"{"setup":{"thinking_choice":"custom"}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), None);

        // padded tokens
        write_config(temp.path(), br#"{"setup":{"thinking_choice":" local "}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), None);
        write_config(temp.path(), br#"{"setup":{"thinking_choice":"byo "}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), None);

        // "byo"
        write_config(temp.path(), br#"{"setup":{"thinking_choice":"byo"}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), Some(SavedLane::Byo));

        // "spp"
        write_config(temp.path(), br#"{"setup":{"thinking_choice":"spp"}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), Some(SavedLane::Spp));

        // "local"
        write_config(temp.path(), br#"{"setup":{"thinking_choice":"local"}}"#);
        assert_eq!(saved_thinking_lane(temp.path()), Some(SavedLane::Local));
    }

    #[test]
    fn daily_input_cutoffs_are_strict_and_reads_are_pure() {
        let config = |value: Value| -> Map<String, Value> {
            serde_json::from_value(serde_json::json!({"daily_inputs":{"location":{"v1":value}}}))
                .unwrap()
        };
        assert_eq!(
            daily_input_since(&Map::new(), "location", "v1").unwrap(),
            None
        );
        assert_eq!(
            daily_input_since(&config(Value::String("20261005".into())), "location", "v1").unwrap(),
            Some(Date::constant(2026, 10, 5))
        );
        for bad in [
            serde_json::json!("2026-10-05"),
            serde_json::json!("2026105"),
            serde_json::json!("20260231"),
            Value::Null,
            serde_json::json!(20261005),
        ] {
            assert!(daily_input_since(&config(bad), "location", "v1").is_err());
        }
        for bad in [
            serde_json::json!({"daily_inputs":false}),
            serde_json::json!({"daily_inputs":{"location":false}}),
        ] {
            let bad: Map<String, Value> = serde_json::from_value(bad).unwrap();
            assert!(daily_input_since(&bad, "location", "v1").is_err());
        }
        let temp = TempDir::new();
        assert!(read_journal_config(temp.path()).unwrap().config.is_none());
        assert!(!temp.path().join("config/journal.json").exists());
    }
}
