// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

pub const BUSY_EXIT_CODE: u8 = 75;

pub struct InstallLease {
    _file: File,
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
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
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
        match file.try_lock() {
            Ok(()) => return Ok(Some(InstallLease { _file: file })),
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
