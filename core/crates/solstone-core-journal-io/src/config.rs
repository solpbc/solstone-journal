// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict, lock-guarded journal configuration mutation.

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::atomic::{JsonWriteOptions, write_json};
use crate::errors::{AtomicWriteError, LockError};
use crate::locking::{LockOptions, hold_lock};

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

/// Strict-load failure for an existing `config/journal.json`.
///
/// This intentionally carries no decoded map, mutation value, or transaction;
/// callers cannot obtain a writable configuration from a failed load.
///
/// ```compile_fail
/// use solstone_core_journal_io::ConfigLoadError;
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

/// Return the canonical config path below `journal_path`.
pub fn get_journal_config_path(journal_path: &Path) -> PathBuf {
    journal_path.join("config").join("journal.json")
}

/// Strictly load, mutate, and conditionally atomically write journal config.
///
/// Missing config materializes an exact clone of `defaults`. Existing config is
/// authoritative and is not merged with defaults. The mutator receives the only
/// mutable map, exclusively after strict load has succeeded under the sidecar
/// lock.
pub fn mutate_journal_config<T, F>(
    journal_path: &Path,
    defaults: &Map<String, Value>,
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
            ..LockOptions::default()
        },
    )
    .map_err(ConfigMutationError::Lock)?;
    let materialized = !config_path.exists();
    let mut config = if materialized {
        defaults.clone()
    } else {
        load_existing_config(&config_path).map_err(ConfigMutationError::Load)?
    };
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

fn load_existing_config(path: &Path) -> Result<Map<String, Value>, ConfigLoadError> {
    let contents = fs::read_to_string(path).map_err(|source| corrupt(path, source))?;
    let value = serde_json::from_str::<Value>(&contents).map_err(|source| corrupt(path, source))?;
    value.as_object().cloned().ok_or_else(|| {
        corrupt(
            path,
            io::Error::new(
                io::ErrorKind::InvalidData,
                "journal config must be a JSON object",
            ),
        )
    })
}

fn corrupt(path: &Path, source: impl Error + Send + Sync + 'static) -> ConfigLoadError {
    ConfigLoadError::Corrupt {
        path: path.to_path_buf(),
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::test_support::TempDir;

    fn defaults() -> Map<String, Value> {
        Map::from_iter([
            ("known".to_owned(), json!("default")),
            ("only_default".to_owned(), json!(true)),
        ])
    }

    #[test]
    fn missing_config_materializes_the_supplied_defaults() {
        let temporary = TempDir::new();
        let result = mutate_journal_config(temporary.path(), &defaults(), |config| {
            assert_eq!(config.get("known"), Some(&json!("default")));
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
        assert_eq!(value, Value::Object(defaults()));
    }

    #[test]
    fn existing_config_is_authoritative_without_default_merge() {
        let temporary = TempDir::new();
        let path = get_journal_config_path(temporary.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"known\":\"existing\"}\n").unwrap();

        let result = mutate_journal_config(temporary.path(), &defaults(), |config| {
            assert_eq!(config.get("known"), Some(&json!("existing")));
            assert!(!config.contains_key("only_default"));
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

        let error = mutate_journal_config(temporary.path(), &defaults(), |_config| {
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "I couldn't read your settings file at {}. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.",
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

        mutate_journal_config(temporary.path(), &defaults(), |config| {
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

        let result = mutate_journal_config(temporary.path(), &defaults(), |_config| {
            JournalConfigMutation {
                changed: false,
                value: (),
            }
        })
        .unwrap();
        assert!(!result.written);
        assert_eq!(fs::metadata(path).unwrap().ino(), inode);
    }
}
