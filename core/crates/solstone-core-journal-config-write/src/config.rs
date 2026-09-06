// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict, lock-guarded journal configuration mutation.

use std::error::Error;
use std::fmt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use solstone_core_journal_config::{
    ConfigLoadError, get_journal_config_path, load_mutation_base, materialized_defaults,
    read_journal_config,
};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, LockTimeout, hold_lock, write_json,
};

use crate::{CommitConfigError, ConfigExpectation, commit_journal_config};

const CAS_CONFLICT_RETRY_BUDGET: Duration = Duration::from_millis(400);
const CAS_CONFLICT_BACKOFF_INITIAL: Duration = Duration::from_millis(5);
const CAS_CONFLICT_BACKOFF_MAX: Duration = Duration::from_millis(40);

/// A caller's requested mutation and return value.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalConfigMutation<T> {
    /// Whether the existing configuration was changed.
    pub changed: bool,
    /// Caller-defined result produced while the configuration was writable.
    pub value: T,
}

/// Successful outcome of one serialized journal configuration transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalConfigTransaction<T> {
    /// Caller-defined result from the mutation.
    pub value: T,
    /// Whether the mutation reported a change.
    pub changed: bool,
    /// Whether this transaction materialized or rewrote the file.
    pub written: bool,
}

/// Failure before a config transaction successfully returns its result.
#[derive(Debug)]
pub enum ConfigMutationError {
    /// The stable config sidecar lock could not be acquired.
    Lock(LockError),
    /// An existing config could not be loaded; no map was supplied to the mutator.
    Load(ConfigLoadError),
    /// Atomic publication of a materialized or modified config failed.
    Write(AtomicWriteError),
}

impl fmt::Display for ConfigMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(error) => error.fmt(formatter),
            Self::Load(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for ConfigMutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lock(error) => Some(error),
            Self::Load(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// Failure before a compare-and-swap config transaction successfully returns.
#[derive(Debug)]
pub enum CasConfigMutationError {
    /// The shared transaction deadline or a config sidecar lock acquisition expired.
    Timeout(LockTimeout),
    /// An existing config could not be loaded; no map was supplied to the mutator.
    Load(ConfigLoadError),
    /// A non-timeout config sidecar lock acquisition failed.
    Lock(LockError),
    /// Atomic publication of a materialized or modified config failed.
    Write(AtomicWriteError),
}

impl fmt::Display for CasConfigMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(error) => error.fmt(formatter),
            Self::Load(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for CasConfigMutationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Timeout(error) => Some(error),
            Self::Load(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// Strictly load, mutate, and conditionally atomically write journal config.
///
/// Missing config materializes canonical defaults. Existing config is
/// authoritative and is not merged with defaults. The mutator receives the only
/// mutable map, exclusively after strict load has succeeded under the sidecar lock.
pub fn mutate_journal_config<T, F>(
    journal_path: &Path,
    lock_options: LockOptions,
    mutator: F,
) -> Result<JournalConfigTransaction<T>, ConfigMutationError>
where
    F: FnOnce(&mut Map<String, Value>) -> JournalConfigMutation<T>,
{
    let config_path = get_journal_config_path(journal_path);
    let _lock = hold_lock(
        &config_path,
        LockOptions {
            mode: Some(0o600),
            ..lock_options
        },
    )
    .map_err(ConfigMutationError::Lock)?;
    let mutation_base = load_mutation_base(journal_path).map_err(ConfigMutationError::Load)?;
    let materialized = mutation_base.materialized;
    let mut config = mutation_base.config;
    let mutation = mutator(&mut config);
    let written = materialized || mutation.changed;
    if written {
        write_json(
            &config_path,
            &config,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(ConfigMutationError::Write)?;
    }
    Ok(JournalConfigTransaction {
        value: mutation.value,
        changed: mutation.changed,
        written,
    })
}

/// Strictly read, mutate, and compare-and-swap journal config until a short deadline.
///
/// The mutator may run more than once after a concurrent replacement. It therefore
/// must not have externally visible side effects. Existing callers needing their
/// mutation to execute under the sidecar lock must continue to use
/// [`mutate_journal_config`].
pub fn mutate_journal_config_cas<T, F>(
    journal_path: &Path,
    mutator: F,
) -> Result<JournalConfigTransaction<T>, CasConfigMutationError>
where
    F: FnMut(&mut Map<String, Value>) -> JournalConfigMutation<T>,
{
    let config_path = get_journal_config_path(journal_path);
    let deadline = Instant::now() + CAS_CONFLICT_RETRY_BUDGET;
    let mut previous_timeout: Option<Duration> = None;
    let mut attempt = 0_u32;
    let mut mutator = mutator;

    loop {
        if attempt > 0 && Instant::now() >= deadline {
            return Err(timeout(&config_path));
        }

        let read = read_journal_config(journal_path).map_err(CasConfigMutationError::Load)?;
        let present = read.present;
        let expected = match read.sha256 {
            Some(fingerprint) => ConfigExpectation::Sha256(fingerprint),
            None => ConfigExpectation::Absent,
        };
        let mut config = read.config.unwrap_or_else(materialized_defaults);
        let mutation = mutator(&mut config);
        let written = !present || mutation.changed;
        if !written {
            return Ok(JournalConfigTransaction {
                value: mutation.value,
                changed: mutation.changed,
                written: false,
            });
        }

        let Some(mut lock_timeout) = deadline.checked_duration_since(Instant::now()) else {
            return Err(timeout(&config_path));
        };
        if let Some(previous) = previous_timeout {
            lock_timeout = lock_timeout.min(previous.saturating_sub(Duration::from_millis(1)));
        }
        if lock_timeout < Duration::from_millis(1) {
            return Err(timeout(&config_path));
        }

        match commit_journal_config(
            journal_path,
            expected,
            &config,
            LockOptions {
                timeout: lock_timeout,
                ..LockOptions::default()
            },
        ) {
            Ok(()) => {
                return Ok(JournalConfigTransaction {
                    value: mutation.value,
                    changed: mutation.changed,
                    written: true,
                });
            }
            Err(CommitConfigError::Conflict(_)) => {
                previous_timeout = Some(lock_timeout);
                attempt += 1;
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    return Err(timeout(&config_path));
                };
                let exponent = attempt.saturating_sub(1).min(3);
                let backoff = CAS_CONFLICT_BACKOFF_INITIAL
                    .checked_mul(1_u32 << exponent)
                    .unwrap_or(CAS_CONFLICT_BACKOFF_MAX)
                    .min(CAS_CONFLICT_BACKOFF_MAX);
                thread::sleep(backoff.min(remaining));
            }
            Err(CommitConfigError::Lock(LockError::Timeout(error))) => {
                return Err(CasConfigMutationError::Timeout(error));
            }
            Err(CommitConfigError::Lock(error)) => return Err(CasConfigMutationError::Lock(error)),
            Err(CommitConfigError::Load(error)) => return Err(CasConfigMutationError::Load(error)),
            Err(CommitConfigError::Write(error)) => {
                return Err(CasConfigMutationError::Write(error));
            }
        }
    }
}

fn timeout(config_path: &Path) -> CasConfigMutationError {
    CasConfigMutationError::Timeout(LockTimeout {
        path: config_path.to_path_buf(),
        timeout: CAS_CONFLICT_RETRY_BUDGET,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use solstone_core_journal_io::hold_lock;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn missing_config_materializes_canonical_defaults() {
        let temporary = TempDir::new();
        let result = mutate_journal_config(temporary.path(), LockOptions::default(), |config| {
            assert_eq!(config["retention"]["raw_media"], json!("keep"));
            JournalConfigMutation {
                changed: false,
                value: "created",
            }
        })
        .unwrap();

        assert_eq!(result.value, "created");
        assert!(!result.changed);
        assert!(result.written);
        let value: Value =
            serde_json::from_slice(&fs::read(get_journal_config_path(temporary.path())).unwrap())
                .unwrap();
        assert_eq!(value["retention"]["raw_media"], json!("keep"));
        assert!(value.get("setup").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn materialized_config_is_private_and_pretty_printed() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new();
        mutate_journal_config(temporary.path(), LockOptions::default(), |_config| {
            JournalConfigMutation {
                changed: false,
                value: (),
            }
        })
        .unwrap();
        let path = get_journal_config_path(temporary.path());
        let contents = fs::read_to_string(&path).unwrap();

        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(contents.contains("\n  \"identity\": "));
    }

    #[test]
    fn existing_config_is_authoritative_without_default_merge() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"known\":\"existing\"}\n").unwrap();

        let result = mutate_journal_config(temporary.path(), LockOptions::default(), |config| {
            assert_eq!(config.get("known"), Some(&json!("existing")));
            assert!(!config.contains_key("identity"));
            JournalConfigMutation {
                changed: false,
                value: (),
            }
        })
        .unwrap();
        assert!(!result.written);
    }

    #[test]
    fn corrupt_config_has_owner_voice_and_is_unchanged() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{not valid json\n";
        fs::write(&path, original).unwrap();

        let error = mutate_journal_config(temporary.path(), LockOptions::default(), |_config| {
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "your settings file at {} couldn't be read. your settings were NOT changed. repair the file or restore config/journal.json from a backup, then try again.",
                path.display()
            )
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn mutation_preserves_unknown_top_level_keys() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            b"{\"known\":\"before\",\"unknown\":{\"keep\":true}}\n",
        )
        .unwrap();

        mutate_journal_config(temporary.path(), LockOptions::default(), |config| {
            config.insert("known".to_owned(), json!("after"));
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap();
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["known"], json!("after"));
        assert_eq!(value["unknown"], json!({"keep": true}));
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_existing_config_is_not_replaced() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"known\":\"existing\"}\n").unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        let result = mutate_journal_config(temporary.path(), LockOptions::default(), |_config| {
            JournalConfigMutation {
                changed: false,
                value: (),
            }
        })
        .unwrap();
        assert!(!result.written);
        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    }

    #[test]
    fn cas_materializes_missing_config_when_mutator_reports_no_change() {
        let temporary = TempDir::new();

        let result = mutate_journal_config_cas(temporary.path(), |_config| JournalConfigMutation {
            changed: false,
            value: "created",
        })
        .unwrap();

        assert_eq!(result.value, "created");
        assert!(!result.changed);
        assert!(result.written);
        assert!(get_journal_config_path(temporary.path()).exists());
    }

    #[test]
    fn cas_same_value_returns_without_waiting_for_held_lock() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"name\":\"same\"}\n").unwrap();
        let lock = hold_lock(&path, LockOptions::default()).unwrap();

        let result = mutate_journal_config_cas(temporary.path(), |config| JournalConfigMutation {
            changed: false,
            value: config["name"].clone(),
        })
        .unwrap();

        assert_eq!(result.value, json!("same"));
        assert!(!result.written);
        assert_eq!(lock.path(), path);
    }

    #[test]
    fn cas_changed_value_times_out_while_lock_is_held() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"name\":\"before\"}\n").unwrap();
        let _lock = hold_lock(&path, LockOptions::default()).unwrap();

        let error = mutate_journal_config_cas(temporary.path(), |config| {
            config.insert("name".to_owned(), json!("after"));
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap_err();

        assert!(matches!(error, CasConfigMutationError::Timeout(_)));
        assert_eq!(fs::read(&path).unwrap(), b"{\"name\":\"before\"}\n");
    }

    #[test]
    fn cas_retries_after_mutator_injected_conflict_and_preserves_sibling() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"target\":\"before\"}\n").unwrap();
        let mut invocations = 0;

        let result = mutate_journal_config_cas(temporary.path(), |config| {
            invocations += 1;
            if invocations == 1 {
                fs::write(&path, b"{\"target\":\"before\",\"sibling\":true}\n").unwrap();
            }
            config.insert("target".to_owned(), json!("after"));
            JournalConfigMutation {
                changed: true,
                value: invocations,
            }
        })
        .unwrap();

        assert_eq!(invocations, 2);
        assert_eq!(result.value, 2);
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value, json!({"target":"after","sibling":true}));
    }

    #[test]
    fn cas_budget_exhaustion_leaves_latest_concurrent_bytes_unchanged() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"target\":\"before\"}\n").unwrap();
        let mut invocations = 0;
        let mut latest = Vec::new();

        let error = mutate_journal_config_cas(temporary.path(), |config| {
            invocations += 1;
            latest = format!("{{\"concurrent\":{invocations}}}\n").into_bytes();
            fs::write(&path, &latest).unwrap();
            config.insert("target".to_owned(), json!("after"));
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap_err();

        assert!(matches!(error, CasConfigMutationError::Timeout(_)));
        assert!(invocations > 1);
        assert_eq!(fs::read(path).unwrap(), latest);
    }
}
