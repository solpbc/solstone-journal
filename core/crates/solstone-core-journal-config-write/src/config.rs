// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict, lock-guarded journal configuration mutation.

use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_config::{ConfigLoadError, get_journal_config_path, load_mutation_base};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, hold_lock, write_json,
};

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

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

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
}
