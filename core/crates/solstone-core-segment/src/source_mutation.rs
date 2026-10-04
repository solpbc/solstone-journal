// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-keyed journal mutation locks.

use std::io;
use std::path::Path;

use solstone_core_journal_io::{BoundParentLock, FileLock, LockError, LockOptions, hold_lock};

const SOURCE_MUTATION_LOCK_PREFIX: &str = ".source-";
const SOURCE_MUTATION_LOCK_SUFFIX: &str = ".mutation";

/// Hold the persistent mutation lock for one capture source.
///
/// Ingest's `write_envelope` and endpoint agent-memory append hold this lock.
/// It is outermost: segment, retention, and stream-record locks are acquired
/// inside it, never before it.
pub fn hold_source_mutation(journal_root: &Path, source: &str) -> Result<FileLock, LockError> {
    hold_source_mutation_with_options(journal_root, source, LockOptions::default())
}

/// Hold the same source sidecar for an agent-memory source under a bound parent.
/// A substituted stream registry cannot redirect even the lock's creation.
pub fn hold_agent_memory_mutation(
    journal: &Path,
    source: &str,
) -> Result<BoundParentLock, LockError> {
    use solstone_core_journal_io::{JournalRoot, acquire_existing_parent_lock_bound};
    use std::ffi::OsStr;
    if source.len() != 64
        || !source
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(invalid_source_error(journal));
    }
    let map_error = |error: String| LockError::Io {
        path: journal.join("streams"),
        source: io::Error::other(error),
    };
    let root = JournalRoot::open(journal).map_err(|error| map_error(error.to_string()))?;
    #[cfg(unix)]
    let directory = solstone_core_journal_io::create_or_open_flat_directory_bound(
        &root,
        OsStr::new("streams"),
        0o700,
        root.canonical_path(),
    )
    .map_err(|error| map_error(error.to_string()))?;
    #[cfg(windows)]
    let directory = solstone_core_journal_io::create_or_open_windows_flat_directory_bound(
        &root,
        OsStr::new("streams"),
        root.canonical_path(),
    )
    .map_err(|error| map_error(error.to_string()))?;
    let name = format!("{SOURCE_MUTATION_LOCK_PREFIX}{source}{SOURCE_MUTATION_LOCK_SUFFIX}.lock");
    let options = LockOptions::default();
    acquire_existing_parent_lock_bound(
        &directory,
        OsStr::new(&name),
        options.timeout,
        options.poll_interval,
    )
    .map_err(|error| map_error(error.to_string()))
}

fn hold_source_mutation_with_options(
    journal_root: &Path,
    source: &str,
    options: LockOptions,
) -> Result<FileLock, LockError> {
    if !is_safe_source(source) {
        return Err(invalid_source_error(journal_root));
    }
    let target = journal_root.join("streams").join(format!(
        "{SOURCE_MUTATION_LOCK_PREFIX}{source}{SOURCE_MUTATION_LOCK_SUFFIX}"
    ));
    hold_lock(target, options)
}

fn is_safe_source(source: &str) -> bool {
    if source.len() > 64 || source.as_bytes().contains(&0) || source.contains(['/', '\\', '.']) {
        return false;
    }
    let mut bytes = source.bytes();
    match bytes.next() {
        None => true,
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        }),
        Some(_) => false,
    }
}

fn invalid_source_error(journal_root: &Path) -> LockError {
    LockError::Io {
        path: journal_root.join("streams"),
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            "source must be a safe capture-source component",
        ),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::io::ErrorKind;
    use std::time::Duration;

    use super::*;
    use crate::test_support::TempDir;

    fn short_options() -> LockOptions {
        LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        }
    }

    #[test]
    fn source_lock_creates_the_expected_persistent_sidecar() {
        let temporary = TempDir::new();
        let target = temporary.path().join("streams/.source-location.mutation");

        let guard = hold_source_mutation(temporary.path(), "location").unwrap();

        assert_eq!(guard.path(), target);
        assert!(fs::exists(target.with_file_name(".source-location.mutation.lock")).unwrap());
    }

    #[test]
    fn same_source_contends_on_the_same_sidecar() {
        let temporary = TempDir::new();
        let _held = hold_source_mutation(temporary.path(), "location").unwrap();

        let result =
            hold_source_mutation_with_options(temporary.path(), "location", short_options());

        assert!(matches!(result, Err(LockError::Timeout(_))));
    }

    #[test]
    fn different_sources_lock_independently() {
        let temporary = TempDir::new();
        let location = hold_source_mutation(temporary.path(), "location").unwrap();
        let audio = hold_source_mutation(temporary.path(), "audio").unwrap();

        assert_ne!(location.path(), audio.path());
    }

    #[test]
    fn unsafe_source_is_rejected_without_creating_streams() {
        let temporary = TempDir::new();
        let journal = temporary.path().join("uncreated-journal");

        for source in ["a/b", "a\\b", "a.b", "Upper", "a\0b"] {
            match hold_source_mutation(&journal, source) {
                Err(LockError::Io { path, source }) => {
                    assert_eq!(path, journal.join("streams"));
                    assert_eq!(source.kind(), ErrorKind::InvalidInput);
                }
                other => panic!("expected invalid source refusal, got {other:?}"),
            }
        }
        assert!(!journal.exists());
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn agent_memory_source_lock_refuses_a_substituted_registry_without_writing_outside() {
        let journal = TempDir::new();
        let outside = TempDir::new();
        std::os::unix::fs::symlink(outside.path(), journal.path().join("streams")).unwrap();
        assert!(hold_agent_memory_mutation(journal.path(), &"a".repeat(64)).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
