// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable sidecar advisory locks.

use std::fs::{self, File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};

use crate::errors::{LockError, LockTimeout};

/// Python-compatible default lock-acquisition timeout.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// Python-compatible maximum lock poll interval.
pub const DEFAULT_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Sidecar lock acquisition options.
#[derive(Debug, Clone, Copy)]
pub struct LockOptions {
    /// Maximum time to wait for the advisory lock.
    pub timeout: Duration,
    /// Upper bound for each randomized retry delay.
    pub poll_interval: Duration,
    /// Sidecar file mode at creation.
    pub mode: Option<u32>,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_LOCK_TIMEOUT,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: None,
        }
    }
}

/// An exclusive `flock(2)` guard. Dropping it releases the lock.
#[derive(Debug)]
pub struct FileLock {
    _guard: Flock<File>,
    path: PathBuf,
}

impl FileLock {
    /// The protected path, rather than the sidecar lock path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Acquire the stable `path.name + ".lock"` sidecar with bounded jittered polling.
///
/// The kernel releases the advisory lock automatically when this process dies,
/// because the RAII guard owns the locked file descriptor.
pub fn hold_lock(path: impl AsRef<Path>, options: LockOptions) -> Result<FileLock, LockError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let file_name = path.file_name().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "lock path has no file name"),
        )
    })?;
    let sidecar = parent.join(format!("{}.lock", file_name.to_string_lossy()));
    let mut open_options = OpenOptions::new();
    open_options.write(true).create(true);
    #[cfg(unix)]
    open_options.mode(options.mode.unwrap_or(0o666));
    let deadline = Instant::now() + options.timeout;
    loop {
        let file = open_options
            .open(&sidecar)
            .map_err(|source| io_error(path, source))?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(guard) => {
                return Ok(FileLock {
                    _guard: guard,
                    path: path.to_path_buf(),
                });
            }
            Err((file, Errno::EACCES | Errno::EAGAIN)) => {
                drop(file);
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(LockTimeout {
                        path: path.to_path_buf(),
                        timeout: options.timeout,
                    }));
                }
                thread::sleep(retry_delay(options.poll_interval));
            }
            Err((file, error)) => {
                drop(file);
                return Err(io_error(path, io::Error::from_raw_os_error(error as i32)));
            }
        }
    }
}

fn retry_delay(poll_interval: Duration) -> Duration {
    let sleep_max = if poll_interval.is_zero() {
        DEFAULT_LOCK_POLL_INTERVAL
    } else {
        poll_interval.min(DEFAULT_LOCK_POLL_INTERVAL)
    };
    let sleep_min = Duration::from_millis(10).min(sleep_max);
    if sleep_min >= sleep_max {
        return sleep_max;
    }
    let span = sleep_max - sleep_min;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let entropy = now ^ u128::from(std::process::id());
    let offset = entropy % (span.as_nanos() + 1);
    sleep_min + Duration::from_nanos(offset as u64)
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn io_error(path: &Path, source: io::Error) -> LockError {
    LockError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn timeout_reports_the_protected_path_and_timeout() {
        let temporary = TempDir::new();
        let path = temporary.path().join("config.json");
        let _first = hold_lock(&path, LockOptions::default()).unwrap();
        let options = LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        };

        let error = hold_lock(&path, options).unwrap_err();
        match error {
            LockError::Timeout(timeout) => {
                assert_eq!(timeout.path, path);
                assert_eq!(timeout.timeout, Duration::from_millis(50));
            }
            LockError::Io { .. } => panic!("expected timeout"),
        }
    }

    #[test]
    fn lock_pause_helper() {
        let Ok(path) = std::env::var("JOURNAL_IO_HELPER_LOCK_PATH") else {
            return;
        };
        let lock = hold_lock(path, LockOptions::default()).unwrap();
        let marker = std::env::var("JOURNAL_IO_TEST_MARKER").unwrap();
        fs::write(marker, "locked").unwrap();
        let _keep_guard_alive = lock;
        loop {
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn lock_is_released_when_the_holder_dies() {
        let temporary = TempDir::new();
        let path = temporary.path().join("config.json");
        let marker = temporary.path().join("locked.ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "locking::tests::lock_pause_helper",
                "--nocapture",
            ])
            .env("JOURNAL_IO_HELPER_LOCK_PATH", &path)
            .env("JOURNAL_IO_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "helper did not acquire the lock");
        let contention = hold_lock(
            &path,
            LockOptions {
                timeout: Duration::from_millis(100),
                ..LockOptions::default()
            },
        );
        assert!(
            matches!(contention, Err(LockError::Timeout(_))),
            "child did not create real lock contention"
        );
        kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
        child.wait().unwrap();

        let started = Instant::now();
        let _lock = hold_lock(
            &path,
            LockOptions {
                timeout: Duration::from_secs(1),
                ..LockOptions::default()
            },
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));
    }
}
