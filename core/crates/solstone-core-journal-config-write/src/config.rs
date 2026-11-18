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
    ConfigLoadError, DailyInputSinceError, get_journal_config_path, load_mutation_base,
    materialized_defaults, parse_daily_input_day, read_journal_config,
};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, LockTimeout, hold_lock_checked,
    write_json_checked,
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

/// Why an input/version's cutoff could not be initialized.
#[derive(Debug)]
pub enum InsertDailyInputSinceError {
    Caller(DailyInputSinceError),
    Stored(DailyInputSinceError),
    Mutation(ConfigMutationError),
}
impl fmt::Display for InsertDailyInputSinceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Caller(error) | Self::Stored(error) => error.fmt(formatter),
            Self::Mutation(error) => error.fmt(formatter),
        }
    }
}
impl Error for InsertDailyInputSinceError {}

/// Initialize one registered input/version once, preserving all sibling cutoffs.
/// Invalid stored state is refused without rewriting it.
pub fn insert_daily_input_since(
    journal: &Path,
    id: &str,
    version: &str,
    day: &str,
) -> Result<String, InsertDailyInputSinceError> {
    insert_daily_input_since_checked(journal, id, version, day, &|| Ok(()))
}

pub fn insert_daily_input_since_checked(
    journal: &Path,
    id: &str,
    version: &str,
    day: &str,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<String, InsertDailyInputSinceError> {
    if id.is_empty() || version.is_empty() {
        return Err(InsertDailyInputSinceError::Caller(DailyInputSinceError));
    }
    let date = parse_daily_input_day(day).map_err(InsertDailyInputSinceError::Caller)?;
    let canonical = solstone_core_time::day_key(date);
    let transaction =
        mutate_journal_config_checked(journal, LockOptions::default(), checkpoint, |config| {
            let stored = solstone_core_journal_config::daily_input_since(config, id, version);
            match stored {
                Err(error) => JournalConfigMutation {
                    changed: false,
                    value: Err(InsertDailyInputSinceError::Stored(error)),
                },
                Ok(Some(date)) => JournalConfigMutation {
                    changed: false,
                    value: Ok(solstone_core_time::day_key(date)),
                },
                Ok(None) => {
                    let inputs = config
                        .entry("daily_inputs")
                        .or_insert_with(|| Value::Object(Map::new()))
                        .as_object_mut()
                        .expect("validated input map");
                    let versions = inputs
                        .entry(id)
                        .or_insert_with(|| Value::Object(Map::new()))
                        .as_object_mut()
                        .expect("validated version map");
                    versions.insert(version.to_owned(), Value::String(canonical.clone()));
                    JournalConfigMutation {
                        changed: true,
                        value: Ok(canonical.clone()),
                    }
                }
            }
        })
        .map_err(InsertDailyInputSinceError::Mutation)?;
    transaction.value
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
    mutate_journal_config_checked(journal_path, lock_options, &|| Ok(()), mutator)
}

pub fn mutate_journal_config_checked<T, F>(
    journal_path: &Path,
    lock_options: LockOptions,
    checkpoint: &dyn Fn() -> Result<(), String>,
    mutator: F,
) -> Result<JournalConfigTransaction<T>, ConfigMutationError>
where
    F: FnOnce(&mut Map<String, Value>) -> JournalConfigMutation<T>,
{
    let config_path = get_journal_config_path(journal_path);
    #[cfg(any(unix, windows))]
    let budget = crate::credential_interlock::PublicationBudget::new(lock_options);
    #[cfg(any(unix, windows))]
    let admitted = crate::credential_interlock::existing_root(journal_path).map_err(|source| {
        ConfigMutationError::Write(AtomicWriteError::Io {
            path: config_path.clone(),
            source,
        })
    })?;
    let entry_checkpoint = || {
        checkpoint()?;
        #[cfg(any(unix, windows))]
        if let Some(root) = &admitted {
            root.revalidate_canonical_binding()
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    };
    let _lock = hold_lock_checked(
        &config_path,
        LockOptions {
            mode: Some(0o600),
            ..lock_options
        },
        &entry_checkpoint,
    )
    .map_err(ConfigMutationError::Lock)?;
    #[cfg(any(unix, windows))]
    let admitted =
        crate::credential_interlock::created_root(journal_path, admitted).map_err(|source| {
            ConfigMutationError::Write(AtomicWriteError::Io {
                path: config_path.clone(),
                source,
            })
        })?;
    let publication_authority = || {
        checkpoint()?;
        #[cfg(any(unix, windows))]
        admitted
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        Ok(())
    };
    let check = || {
        publication_authority().map_err(|detail| {
            ConfigMutationError::Write(AtomicWriteError::Io {
                path: config_path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::Interrupted, detail),
            })
        })
    };
    check()?;
    let mutation_base = load_mutation_base(journal_path).map_err(ConfigMutationError::Load)?;
    let materialized = mutation_base.materialized;
    let mut config = mutation_base.config;
    #[cfg(any(unix, windows))]
    let original = config.clone();
    let mutation = mutator(&mut config);
    let written = materialized || mutation.changed;
    if written {
        check()?;
        #[cfg(any(unix, windows))]
        let credential_interlock = crate::credential_interlock::acquire(
            &admitted,
            &original,
            &config,
            &budget,
            &publication_authority,
        )
        .map_err(|source| {
            ConfigMutationError::Write(AtomicWriteError::Io {
                path: config_path.clone(),
                source,
            })
        })?;
        let publication_checkpoint = || {
            publication_authority()?;
            #[cfg(any(unix, windows))]
            if let Some(interlock) = &credential_interlock {
                interlock.checkpoint()?;
            }
            Ok(())
        };
        write_json_checked(
            &config_path,
            &config,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
            &publication_checkpoint,
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

    #[test]
    fn first_use_mutator_admits_owner_created_root_before_callback() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        assert!(!journal.exists());
        let result = mutate_journal_config(&journal, LockOptions::default(), |config| {
            assert!(journal.join("config").is_dir());
            config.insert("env".to_owned(), json!({"OPENAI_API_KEY":"synthetic"}));
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap();
        assert!(result.written);
        assert_eq!(
            read_journal_config(&journal).unwrap().config.unwrap()["env"]["OPENAI_API_KEY"],
            "synthetic"
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_root_open_failure_is_not_reinterpreted_as_first_use() {
        use solstone_core_journal_io::{AcquisitionPrimitive, run_with_acquisition_fault};

        let temporary = TempDir::new();
        let missing = fs::symlink_metadata(temporary.path().join("missing")).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        let called = std::cell::Cell::new(false);
        let (result, fired) = run_with_acquisition_fault(
            AcquisitionPrimitive::RequestedRootOpen,
            1,
            missing.raw_os_error().unwrap(),
            || {
                mutate_journal_config(temporary.path(), LockOptions::default(), |_config| {
                    called.set(true);
                    JournalConfigMutation {
                        changed: true,
                        value: (),
                    }
                })
            },
        );
        assert!(fired);
        assert!(result.is_err());
        assert!(!called.get());
        assert!(!temporary.path().join("config").exists());
        assert!(!temporary.path().join("health").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_root_entry_is_not_an_absent_first_use() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("journal");
        let target = temporary.path().join("missing-target");
        std::os::unix::fs::symlink(&target, &journal).unwrap();
        let called = std::cell::Cell::new(false);
        let result = mutate_journal_config(&journal, LockOptions::default(), |_config| {
            called.set(true);
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        });
        assert!(result.is_err());
        assert!(!called.get());
        assert!(fs::symlink_metadata(&journal).unwrap().is_symlink());
        assert!(!target.exists());
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
    fn credential_interlock_does_not_create_health_after_legacy_mutator_root_replacement() {
        for initially_missing in [false, true] {
            let temporary = TempDir::new();
            let journal = temporary.path().join("journal");
            let displaced = temporary.path().join("displaced");
            let original = b"{\"env\":{}}\n";
            let replacement = b"{\"replacement\":true}\n";
            if !initially_missing {
                fs::create_dir_all(journal.join("config")).unwrap();
                fs::write(journal.join("config/journal.json"), original).unwrap();
            }
            let called = std::cell::Cell::new(false);
            let replaced = std::cell::Cell::new(false);
            let result = mutate_journal_config(&journal, LockOptions::default(), |config| {
                called.set(true);
                match fs::rename(&journal, &displaced) {
                    Ok(()) => {
                        replaced.set(true);
                        fs::create_dir_all(journal.join("config")).unwrap();
                        fs::write(journal.join("config/journal.json"), replacement).unwrap();
                    }
                    // Windows refuses to rename a root while the mutation holds
                    // a handle inside it, so the replacement cannot begin.
                    Err(error)
                        if cfg!(windows)
                            && error.kind() == std::io::ErrorKind::PermissionDenied => {}
                    Err(error) => panic!("stage the replacement: {error}"),
                }
                config.insert("env".to_owned(), json!({"OPENAI_API_KEY":"synthetic"}));
                JournalConfigMutation {
                    changed: true,
                    value: (),
                }
            });
            assert!(called.get(), "the original owner creates a missing root");
            if !replaced.get() {
                result.expect("the mutation still names the only root");
                assert!(!displaced.exists());
                let written = fs::read_to_string(journal.join("config/journal.json")).unwrap();
                assert!(written.contains("synthetic"));
                continue;
            }
            assert!(result.is_err());
            assert!(!journal.join("health").exists());
            assert!(!displaced.join("health").exists());
            assert_eq!(
                fs::read(journal.join("config/journal.json")).unwrap(),
                replacement
            );
            if initially_missing {
                assert!(!displaced.join("config/journal.json").exists());
            } else {
                assert_eq!(
                    fs::read(displaced.join("config/journal.json")).unwrap(),
                    original
                );
            }
        }
    }

    #[test]
    fn zero_timeout_credential_mutation_preserves_the_uncontended_attempt() {
        let temporary = TempDir::new();
        let result = mutate_journal_config(
            temporary.path(),
            LockOptions {
                timeout: std::time::Duration::ZERO,
                ..LockOptions::default()
            },
            |config| {
                config.insert(
                    "env".to_owned(),
                    json!({"OPENAI_API_KEY":"synthetic-zero-wait"}),
                );
                JournalConfigMutation {
                    changed: true,
                    value: (),
                }
            },
        )
        .unwrap();
        assert!(result.written);
        assert_eq!(
            read_journal_config(temporary.path())
                .unwrap()
                .config
                .unwrap()["env"]["OPENAI_API_KEY"],
            "synthetic-zero-wait"
        );
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
                "your settings file at {} couldn't be read. your settings were not changed. repair the file or restore config/journal.json from a backup, then try again.",
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

    #[cfg(all(test, feature = "full-tests"))]
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
    fn checked_mutation_refuses_at_pre_rename_and_keeps_the_destination() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{\"known\":\"existing\"}\n";
        fs::write(&path, original).unwrap();
        let parent = path.parent().unwrap().to_path_buf();
        let error = mutate_journal_config_checked(
            temporary.path(),
            LockOptions::default(),
            &|| {
                let staged = fs::read_dir(&parent).unwrap().any(|entry| {
                    solstone_core_journal_io::is_publication_candidate_name(
                        &entry.unwrap().file_name(),
                    )
                });
                if staged {
                    Err("stop before rename".to_owned())
                } else {
                    Ok(())
                }
            },
            |config| {
                config.insert("known".to_owned(), json!("changed"));
                JournalConfigMutation {
                    changed: true,
                    value: (),
                }
            },
        )
        .unwrap_err();
        assert!(matches!(error, ConfigMutationError::Write(_)));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!fs::read_dir(&parent).unwrap().any(|entry| {
            solstone_core_journal_io::is_publication_candidate_name(&entry.unwrap().file_name())
        }));
    }

    #[cfg(all(test, feature = "full-tests"))]
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

    #[test]
    fn insert_rejects_a_bad_caller_date_and_a_bad_stored_date_without_writing() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        assert!(
            insert_daily_input_since(
                temporary.path(),
                "location",
                "location-words-1",
                "2026-10-05"
            )
            .is_err()
        );
        assert!(!path.exists());

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{\"daily_inputs\":{\"location\":{\"location-words-1\":\"20260231\"}},\"keep\":true}\n";
        fs::write(&path, original).unwrap();
        assert!(
            insert_daily_input_since(temporary.path(), "location", "location-words-1", "20261005")
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn two_inserts_persist_one_stored_date() {
        let temporary = TempDir::new();
        let first =
            insert_daily_input_since(temporary.path(), "location", "location-words-1", "20261001")
                .unwrap();
        let second =
            insert_daily_input_since(temporary.path(), "location", "location-words-1", "20261008")
                .unwrap();
        assert_eq!(first, "20261001");
        assert_eq!(second, first);
        let value: Value =
            serde_json::from_slice(&fs::read(get_journal_config_path(temporary.path())).unwrap())
                .unwrap();
        assert_eq!(
            value["daily_inputs"]["location"]["location-words-1"],
            json!("20261001")
        );

        let left = temporary.path().to_path_buf();
        let right = left.clone();
        let first_thread = std::thread::spawn(move || {
            insert_daily_input_since(&left, "location", "location-words-1", "20260101").unwrap()
        });
        let second_thread = std::thread::spawn(move || {
            insert_daily_input_since(&right, "location", "location-words-1", "20260202").unwrap()
        });
        let left_result = first_thread.join().unwrap();
        let right_result = second_thread.join().unwrap();
        assert_eq!(left_result, right_result);
        assert_eq!(left_result, "20261001");
    }
    #[test]
    fn activation_preserves_sibling_inputs_versions_and_first_concurrent_cutoff() {
        let temporary = TempDir::new();
        let journal = temporary.path();
        insert_daily_input_since(journal, "location", "v1", "20261005").unwrap();
        insert_daily_input_since(journal, "other", "v1", "20261006").unwrap();
        insert_daily_input_since(journal, "location", "v2", "20261007").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let threads: Vec<_> = ["20261008", "20261009"]
            .into_iter()
            .map(|day| {
                let journal = journal.to_path_buf();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    insert_daily_input_since(&journal, "new", "v1", day).unwrap()
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results[0], results[1]);
        let config = read_journal_config(journal).unwrap().config.unwrap();
        assert_eq!(config["daily_inputs"]["location"]["v1"], "20261005");
        assert_eq!(config["daily_inputs"]["location"]["v2"], "20261007");
        assert_eq!(config["daily_inputs"]["other"]["v1"], "20261006");
        assert_eq!(
            insert_daily_input_since(journal, "new", "v1", "20261101").unwrap(),
            results[0]
        );
    }
}
