// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Compare-and-swap journal configuration replacement.

use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::{ConfigLoadError, get_journal_config_path, read_journal_config};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};

/// A caller's expected current journal configuration state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigExpectation {
    /// The config file must not exist.
    Absent,
    /// The config file must have this exact `sha256:<lowercase hex>` fingerprint.
    Sha256(String),
}

impl fmt::Display for ConfigExpectation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => formatter.write_str("absent"),
            Self::Sha256(fingerprint) => formatter.write_str(fingerprint),
        }
    }
}

/// The actual current journal configuration state observed under the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFingerprint {
    /// The config file was absent.
    Absent,
    /// The config file was present with this exact fingerprint.
    Sha256(String),
}

impl fmt::Display for ConfigFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => formatter.write_str("absent"),
            Self::Sha256(fingerprint) => formatter.write_str(fingerprint),
        }
    }
}

/// The expected and actual fingerprints from a failed configuration CAS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigConflict {
    /// State supplied by the caller.
    pub expected: ConfigExpectation,
    /// State observed under the sidecar lock.
    pub actual: ConfigFingerprint,
}

impl fmt::Display for ConfigConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "config fingerprint conflict: expected {}, found {}",
            self.expected, self.actual
        )
    }
}

impl Error for ConfigConflict {}

/// Failure before a configuration CAS replacement completes.
#[derive(Debug)]
pub enum CommitConfigError {
    /// The stable config sidecar lock could not be acquired.
    Lock(LockError),
    /// An existing config could not be strictly loaded.
    Load(ConfigLoadError),
    /// The caller's expected fingerprint did not match the locked current state.
    Conflict(ConfigConflict),
    /// Atomic publication of the replacement config failed.
    Write(AtomicWriteError),
}

impl fmt::Display for CommitConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Load(error) => error.fmt(formatter),
            Self::Conflict(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for CommitConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Load(error) => Some(error),
            Self::Conflict(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// Replace a journal config only when its locked fingerprint matches `expected`.
///
/// The replacement is always published on a fingerprint match, including when
/// it serializes to the same bytes as the existing configuration. Callers that
/// can determine no change must skip this operation themselves.
pub fn commit_journal_config(
    journal_path: &Path,
    expected: ConfigExpectation,
    replacement: &Map<String, Value>,
    options: LockOptions,
) -> Result<(), CommitConfigError> {
    let config_path = get_journal_config_path(journal_path);
    let _lock = hold_lock(
        &config_path,
        LockOptions {
            mode: Some(0o600),
            ..options
        },
    )
    .map_err(CommitConfigError::Lock)?;
    let read = read_journal_config(journal_path).map_err(CommitConfigError::Load)?;
    let actual = read
        .sha256
        .map_or(ConfigFingerprint::Absent, ConfigFingerprint::Sha256);
    if !matches_expectation(&expected, &actual) {
        return Err(CommitConfigError::Conflict(ConfigConflict {
            expected,
            actual,
        }));
    }
    write_json(
        &config_path,
        replacement,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(CommitConfigError::Write)
}

fn matches_expectation(expected: &ConfigExpectation, actual: &ConfigFingerprint) -> bool {
    match (expected, actual) {
        (ConfigExpectation::Absent, ConfigFingerprint::Absent) => true,
        (ConfigExpectation::Sha256(expected), ConfigFingerprint::Sha256(actual)) => {
            expected == actual
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use serde_json::{Map, Value, json};
    use solstone_core_journal_config::{get_journal_config_path, read_journal_config};
    use solstone_core_journal_io::{LockError, LockOptions, hold_lock, write_json};

    use super::*;
    use crate::test_support::TempDir;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn write_existing(temporary: &TempDir, value: &Map<String, Value>) -> String {
        let path = get_journal_config_path(temporary.path());
        write_json(&path, value, JsonWriteOptions::default()).unwrap();
        read_journal_config(temporary.path())
            .unwrap()
            .sha256
            .unwrap()
    }

    #[test]
    fn commit_success_on_matching_fingerprint_replaces_with_private_pretty_json() {
        let temporary = TempDir::new();
        let old = object(json!({"known": "before"}));
        let expected = write_existing(&temporary, &old);
        let replacement = object(json!({"known": "after", "unknown": {"keep": true}}));

        commit_journal_config(
            temporary.path(),
            ConfigExpectation::Sha256(expected),
            &replacement,
            LockOptions::default(),
        )
        .unwrap();

        let path = get_journal_config_path(temporary.path());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\n  \"known\": \"after\""));
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
            json!({"known": "after", "unknown": {"keep": true}})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn commit_success_on_matching_fingerprint_rewrites_identical_bytes() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new();
        let replacement = object(json!({"known": "same"}));
        let expected = write_existing(&temporary, &replacement);
        let path = get_journal_config_path(temporary.path());
        let before = fs::metadata(&path).unwrap().ino();

        commit_journal_config(
            temporary.path(),
            ConfigExpectation::Sha256(expected),
            &replacement,
            LockOptions::default(),
        )
        .unwrap();

        assert_ne!(fs::metadata(path).unwrap().ino(), before);
    }

    #[test]
    fn commit_expect_absent_succeeds_for_missing_file() {
        let temporary = TempDir::new();
        let replacement = object(json!({"created": true}));

        commit_journal_config(
            temporary.path(),
            ConfigExpectation::Absent,
            &replacement,
            LockOptions::default(),
        )
        .unwrap();

        let read = read_journal_config(temporary.path()).unwrap();
        assert!(read.present);
        assert_eq!(read.config, Some(replacement));
    }

    #[test]
    fn commit_corrupt_file_is_unchanged_and_returns_load_error() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{not valid json\n";
        fs::write(&path, original).unwrap();

        let error = commit_journal_config(
            temporary.path(),
            ConfigExpectation::Absent,
            &object(json!({"replacement": true})),
            LockOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommitConfigError::Load(ConfigLoadError::Corrupt { .. })
        ));
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn commit_conflict_expect_absent_but_config_now_present_is_untouched() {
        let temporary = TempDir::new();
        let original = b"{\"appeared\":true}\n";
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, original).unwrap();

        let error = commit_journal_config(
            temporary.path(),
            ConfigExpectation::Absent,
            &object(json!({"replacement": true})),
            LockOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommitConfigError::Conflict(ConfigConflict {
                expected: ConfigExpectation::Absent,
                actual: ConfigFingerprint::Sha256(_),
            })
        ));
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn commit_conflict_expect_hash_but_config_now_absent_does_not_materialize() {
        let temporary = TempDir::new();

        let error = commit_journal_config(
            temporary.path(),
            ConfigExpectation::Sha256(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_owned(),
            ),
            &object(json!({"replacement": true})),
            LockOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommitConfigError::Conflict(ConfigConflict {
                expected: ConfigExpectation::Sha256(_),
                actual: ConfigFingerprint::Absent,
            })
        ));
        assert!(!get_journal_config_path(temporary.path()).exists());
    }

    #[test]
    fn commit_conflict_expect_hash_mismatch_leaves_existing_bytes_unchanged() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{\"known\":\"actual\"}\n";
        fs::write(&path, original).unwrap();

        let error = commit_journal_config(
            temporary.path(),
            ConfigExpectation::Sha256(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_owned(),
            ),
            &object(json!({"replacement": true})),
            LockOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CommitConfigError::Conflict(ConfigConflict {
                expected: ConfigExpectation::Sha256(_),
                actual: ConfigFingerprint::Sha256(_),
            })
        ));
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn commit_timeout_reports_the_protected_path_and_timeout() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        let _first = hold_lock(&path, LockOptions::default()).unwrap();
        let options = LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        };

        let error = commit_journal_config(
            temporary.path(),
            ConfigExpectation::Absent,
            &object(json!({"replacement": true})),
            options,
        )
        .unwrap_err();
        match error {
            CommitConfigError::Lock(LockError::Timeout(timeout)) => {
                assert_eq!(timeout.path, path);
                assert_eq!(timeout.timeout, Duration::from_millis(50));
            }
            _ => panic!("expected timeout"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn commit_retains_the_stable_lock_sidecar() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new();
        let first = object(json!({"version": 1}));
        commit_journal_config(
            temporary.path(),
            ConfigExpectation::Absent,
            &first,
            LockOptions::default(),
        )
        .unwrap();
        let config_path = get_journal_config_path(temporary.path());
        let sidecar = config_path.with_file_name("journal.json.lock");
        let inode = fs::metadata(&sidecar).unwrap().ino();
        let expected = read_journal_config(temporary.path())
            .unwrap()
            .sha256
            .unwrap();

        commit_journal_config(
            temporary.path(),
            ConfigExpectation::Sha256(expected),
            &object(json!({"version": 2})),
            LockOptions::default(),
        )
        .unwrap();

        assert!(sidecar.exists());
        assert_eq!(fs::metadata(sidecar).unwrap().ino(), inode);
    }
}
