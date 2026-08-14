// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteObserverMigrationReport {
    pub moved_files: usize,
    pub config_updated: bool,
    pub conflicts: usize,
}
#[derive(Debug)]
pub struct RemoteObserverMigrationError(pub String);
impl std::fmt::Display for RemoteObserverMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for RemoteObserverMigrationError {}
pub fn migrate_remote_observer_storage(
    journal: &Path,
) -> Result<RemoteObserverMigrationReport, RemoteObserverMigrationError> {
    let source = journal.join("apps/remote/remotes");
    let target = journal.join("apps/observer/observers");
    let mut report = RemoteObserverMigrationReport::default();
    if source.exists() {
        let mut files = Vec::new();
        walk(&source, &mut files)?;
        files.sort();
        for file in files {
            let relative = file
                .strip_prefix(&source)
                .map_err(|e| RemoteObserverMigrationError(e.to_string()))?;
            let destination = target.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            if destination.exists() {
                if fs::read(&file).map_err(io)? == fs::read(&destination).map_err(io)? {
                    fs::remove_file(&file).map_err(io)?;
                } else {
                    report.conflicts += 1;
                }
            } else {
                fs::rename(&file, &destination).map_err(io)?;
                report.moved_files += 1;
            }
        }
        prune(&source);
        prune(&journal.join("apps/remote"));
    }
    let transaction = mutate_journal_config(journal, LockOptions::default(), |config| {
        let changed = match config.get_mut("observe").and_then(Value::as_object_mut) {
            Some(observe) => match observe.remove("remote") {
                Some(Value::Object(remote)) => {
                    let observer = observe
                        .entry("observer")
                        .or_insert_with(|| Value::Object(Default::default()));
                    if !observer.is_object() {
                        *observer = Value::Object(Default::default());
                    }
                    let target = observer.as_object_mut().unwrap();
                    for (key, value) in remote {
                        target.entry(key).or_insert(value);
                    }
                    true
                }
                Some(value) => {
                    observe.insert("remote".to_owned(), value);
                    false
                }
                None => false,
            },
            None => false,
        };
        JournalConfigMutation {
            changed,
            value: changed,
        }
    })
    .map_err(|e| RemoteObserverMigrationError(e.to_string()))?;
    report.config_updated = transaction.value;
    Ok(report)
}
fn walk(
    root: &Path,
    files: &mut Vec<std::path::PathBuf>,
) -> Result<(), RemoteObserverMigrationError> {
    for entry in fs::read_dir(root).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path.is_dir() {
            walk(&path, files)?
        } else {
            files.push(path)
        }
    }
    Ok(())
}
fn prune(path: &Path) {
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                prune(&path)
            }
        }
    }
    let _ = fs::remove_dir(path);
}
fn io(error: std::io::Error) -> RemoteObserverMigrationError {
    RemoteObserverMigrationError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn moves_legacy_file_and_merges_observe_config() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("apps/remote/remotes/a.json");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, b"legacy").unwrap();
        let config = temp.path().join("config/journal.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, br#"{"observe":{"remote":{"url":"x"}}}"#).unwrap();
        let report = migrate_remote_observer_storage(temp.path()).unwrap();
        assert_eq!(report.moved_files, 1);
        assert!(temp.path().join("apps/observer/observers/a.json").is_file());
        let value: Value = serde_json::from_slice(&fs::read(config).unwrap()).unwrap();
        assert_eq!(value["observe"]["observer"]["url"], "x");
    }

    #[test]
    fn preserves_different_destination_and_removes_identical_one() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("apps/remote/remotes/a.json");
        let target = temp.path().join("apps/observer/observers/a.json");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&source, b"old").unwrap();
        fs::write(&target, b"new").unwrap();
        let report = migrate_remote_observer_storage(temp.path()).unwrap();
        assert_eq!(report.conflicts, 1);
        assert!(source.exists());
    }
}
