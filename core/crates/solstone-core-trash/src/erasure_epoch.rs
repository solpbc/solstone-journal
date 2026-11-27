// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Monotonic permanent-erasure epoch for invalidating staged journal exports.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::atomic::atomic_replace;
use solstone_core_journal_io::copy_excludes::TRASH_ERASURE_EPOCH_FILE;
use solstone_core_journal_io::locking::{LockOptions, hold_lock};

use crate::TrashError;

const EPOCH_SCHEMA: u32 = 1;

fn is_false(value: &bool) -> bool {
    !*value
}

/// Durable witness of permanent Trash erasures for export invalidation.
///
/// Missing epoch files read as `epoch == 0` with an empty token and
/// `erasing == false`. Every permanent erasure calls [`begin_erasure`] before
/// publishing `State::Erasing` or running hooks/unlinks, advancing `epoch`
/// monotonically, rotating `token`, and setting `erasing = true` until no
/// `State::Erasing` items remain. Callers check invalidation in O(1) via
/// [`read_erasure_epoch`] without scanning `.trash/`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErasureEpoch {
    pub schema: u32,
    pub epoch: u64,
    pub token: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub erasing: bool,
}

impl ErasureEpoch {
    /// Initial epoch observed before any permanent Trash erasure has occurred.
    pub fn initial() -> Self {
        Self {
            schema: EPOCH_SCHEMA,
            epoch: 0,
            token: String::new(),
            erasing: false,
        }
    }
}

/// Read the journal's permanent-erasure epoch.
///
/// Fails closed (`Err`) on symlinks, non-regular files, unreadable bytes, or
/// malformed JSON so callers never mistake a damaged witness for "no erasure".
pub fn read_erasure_epoch(journal: &Path) -> Result<ErasureEpoch, TrashError> {
    let health = journal.join("health");
    match fs::symlink_metadata(&health) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ErasureEpoch::initial());
        }
        Err(error) => return Err(TrashError::Io(error.to_string())),
        Ok(meta) if !meta.file_type().is_dir() => {
            return Err(TrashError::Io(
                "health path is not a plain directory".to_owned(),
            ));
        }
        Ok(_) => {}
    }

    let path = journal.join(TRASH_ERASURE_EPOCH_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ErasureEpoch::initial());
        }
        Err(error) => return Err(TrashError::Io(error.to_string())),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(TrashError::Io(
                "trash erasure epoch path is not a regular file".to_owned(),
            ));
        }
        Ok(_) => {}
    }

    let bytes = fs::read(&path).map_err(|error| TrashError::Io(error.to_string()))?;
    let parsed: ErasureEpoch =
        serde_json::from_slice(&bytes).map_err(|error| TrashError::Io(error.to_string()))?;
    if parsed.schema != EPOCH_SCHEMA
        || parsed.epoch == 0
        || parsed.token.is_empty()
        || !parsed.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(TrashError::Io("invalid trash erasure epoch".to_owned()));
    }
    Ok(parsed)
}

/// Atomically advance the journal's permanent-erasure epoch and mark an
/// erasure in progress BEFORE publishing `State::Erasing`, running kind/backup
/// hooks, or unlinking files.
pub(crate) fn begin_erasure(journal: &Path) -> Result<ErasureEpoch, TrashError> {
    write_epoch_update(journal, true)
}

/// Clear the `erasing` flag once no `State::Erasing` items remain, preserving
/// the monotonic `epoch` and `token` written by [`begin_erasure`].
pub(crate) fn settle_erasure(journal: &Path) -> Result<ErasureEpoch, TrashError> {
    let health = journal.join("health");
    fs::create_dir_all(&health).map_err(|error| TrashError::Io(error.to_string()))?;
    let path = journal.join(TRASH_ERASURE_EPOCH_FILE);
    let _lock = hold_lock(&path, LockOptions::default())
        .map_err(|error| TrashError::Io(error.to_string()))?;

    let current = match read_erasure_epoch(journal) {
        Ok(current) if current.epoch > 0 => current,
        _ => return write_epoch_under_lock(journal, &path, false),
    };
    if !current.erasing {
        return Ok(current);
    }
    let settled = ErasureEpoch {
        erasing: false,
        ..current
    };
    let mut bytes =
        serde_json::to_vec_pretty(&settled).map_err(|error| TrashError::Io(error.to_string()))?;
    bytes.push(b'\n');
    atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|error| TrashError::Io(error.to_string()))?;
    Ok(settled)
}

#[cfg(test)]
pub(crate) fn bump_erasure_epoch(journal: &Path) -> Result<ErasureEpoch, TrashError> {
    write_epoch_update(journal, false)
}

fn write_epoch_update(journal: &Path, erasing: bool) -> Result<ErasureEpoch, TrashError> {
    let health = journal.join("health");
    fs::create_dir_all(&health).map_err(|error| TrashError::Io(error.to_string()))?;
    let path = journal.join(TRASH_ERASURE_EPOCH_FILE);
    let _lock = hold_lock(&path, LockOptions::default())
        .map_err(|error| TrashError::Io(error.to_string()))?;
    write_epoch_under_lock(journal, &path, erasing)
}

fn write_epoch_under_lock(
    journal: &Path,
    path: &Path,
    erasing: bool,
) -> Result<ErasureEpoch, TrashError> {
    let next_epoch = match read_erasure_epoch(journal) {
        Ok(current) => current.epoch.saturating_add(1),
        Err(_) => 1,
    };
    let mut random_bytes = [0u8; 16];
    getrandom::fill(&mut random_bytes).map_err(|error| TrashError::Io(error.to_string()))?;
    let mut token = String::with_capacity(32);
    for byte in random_bytes {
        let _ = write!(&mut token, "{byte:02x}");
    }

    let next = ErasureEpoch {
        schema: EPOCH_SCHEMA,
        epoch: next_epoch,
        token,
        erasing,
    };
    let mut bytes =
        serde_json::to_vec_pretty(&next).map_err(|error| TrashError::Io(error.to_string()))?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|error| TrashError::Io(error.to_string()))?;
    Ok(next)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "unit tests for erasure epoch"
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-trash-epoch-{name}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create test dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_epoch_reads_as_initial_and_bumps_monotonically() {
        let dir = TestDir::new("monotonic");
        assert_eq!(
            read_erasure_epoch(dir.path()).expect("initial"),
            ErasureEpoch::initial()
        );
        let first = bump_erasure_epoch(dir.path()).expect("first bump");
        assert_eq!(first.epoch, 1);
        assert_eq!(first.token.len(), 32);
        assert_eq!(read_erasure_epoch(dir.path()).expect("read first"), first);

        let second = bump_erasure_epoch(dir.path()).expect("second bump");
        assert_eq!(second.epoch, 2);
        assert_ne!(second.token, first.token);
        assert_eq!(read_erasure_epoch(dir.path()).expect("read second"), second);
    }

    #[test]
    fn corrupt_epoch_fails_closed_and_subsequent_bump_recovers_distinct_token() {
        let dir = TestDir::new("corrupt");
        let first = bump_erasure_epoch(dir.path()).expect("first bump");
        fs::write(dir.path().join(TRASH_ERASURE_EPOCH_FILE), b"{not json").expect("corrupt");
        assert!(read_erasure_epoch(dir.path()).is_err());

        let recovered = bump_erasure_epoch(dir.path()).expect("recovered bump");
        assert_ne!(recovered, first);
        assert_eq!(
            read_erasure_epoch(dir.path()).expect("read recovered"),
            recovered
        );
    }
}
