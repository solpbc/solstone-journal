// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const BUSY_EXIT_CODE: u8 = 75;

#[cfg(unix)]
type LeaseLock = nix::fcntl::Flock<File>;
#[cfg(not(unix))]
type LeaseLock = File;

pub struct InstallLease {
    _file: LeaseLock,
}

#[cfg(unix)]
fn try_lock(file: File, shared: bool) -> Result<LeaseLock, TryLockError> {
    use nix::fcntl::{Flock, FlockArg};
    // Android's pinned standard library does not implement File locking.
    // Use the same owned OS lock on Unix so host tests exercise that backend.
    let operation = if shared {
        FlockArg::LockSharedNonblock
    } else {
        FlockArg::LockExclusiveNonblock
    };
    Flock::lock(file, operation).map_err(|(_file, error)| {
        if error == nix::errno::Errno::EWOULDBLOCK {
            TryLockError::WouldBlock
        } else {
            TryLockError::Error(std::io::Error::from(error))
        }
    })
}

#[cfg(not(unix))]
fn try_lock(file: File, shared: bool) -> Result<LeaseLock, TryLockError> {
    if shared {
        file.try_lock_shared()?;
    } else {
        file.try_lock()?;
    }
    Ok(file)
}

pub fn lease_path(journal: &Path, provider: &str) -> PathBuf {
    journal
        .join("health/providers")
        .join(format!("{provider}.lease"))
}

/// Return whether an existing lease is held by taking a momentary shared lock
/// without creating any filesystem state.
///
/// While the shared probe lock is held, a concurrent `acquire()` can observe
/// `EWOULDBLOCK`; the lock drops immediately and acquire retries within its
/// five-attempt, 250 ms window.
pub fn is_held(journal: &Path, provider: &str) -> std::io::Result<bool> {
    let path = lease_path(journal, provider);
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "install lease is not a regular file",
        ));
    }
    match try_lock(file, true) {
        Ok(_lock) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

pub fn acquire(journal: &Path, provider: &str) -> std::io::Result<Option<InstallLease>> {
    let path = lease_path(journal, provider);
    fs::create_dir_all(path.parent().expect("lease parent"))?;
    let deadline = Instant::now() + Duration::from_millis(250);
    for attempt in 0..5 {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        match try_lock(file, false) {
            Ok(lock) => return Ok(Some(InstallLease { _file: lock })),
            Err(TryLockError::WouldBlock) => {
                if attempt == 4 || Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(None)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn lease_probes_are_read_only_and_exclusive_ownership_ends_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        assert!(!is_held(directory.path(), "local").unwrap());
        assert!(!directory.path().join("health").exists());
        let lease = acquire(directory.path(), "local").unwrap().unwrap();
        assert!(is_held(directory.path(), "local").unwrap());
        assert!(acquire(directory.path(), "local").unwrap().is_none());
        drop(lease);
        assert!(!is_held(directory.path(), "local").unwrap());
        assert!(acquire(directory.path(), "local").unwrap().is_some());
    }

    #[test]
    fn shared_observers_coexist_and_only_delay_exclusive_admission() {
        let directory = tempfile::tempdir().unwrap();
        drop(acquire(directory.path(), "local").unwrap().unwrap());
        let path = lease_path(directory.path(), "local");
        let observer = try_lock(File::open(&path).unwrap(), true).unwrap();
        let second = try_lock(File::open(&path).unwrap(), true).unwrap();
        assert!(!is_held(directory.path(), "local").unwrap());
        assert!(acquire(directory.path(), "local").unwrap().is_none());
        drop(observer);
        drop(second);
        assert!(acquire(directory.path(), "local").unwrap().is_some());
    }
}
