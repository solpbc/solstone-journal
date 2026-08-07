// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Advisory file leases held for the lifetime of their guard.

use std::fs::{self, File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::sys::stat::{Mode, fchmod};

use crate::errors::LeaseError;

/// Python-compatible number of nonblocking acquisition attempts.
pub const DEFAULT_LEASE_ATTEMPTS: usize = 5;
/// Python-compatible total retry window, shared by every attempt.
pub const DEFAULT_LEASE_RETRY_MAX: Duration = Duration::from_millis(250);
/// Mode applied to every lease file, including files that predate this acquire.
pub const DEFAULT_LEASE_MODE: u32 = 0o600;

/// Options for acquiring a process-lifetime advisory lease.
#[derive(Debug, Clone, Copy)]
pub struct LeaseOptions {
    /// Number of acquisition attempts; zero is treated as one.
    pub attempts: usize,
    /// Total retry window; the deadline is set once before the first attempt.
    pub retry_max: Duration,
    /// Lease-file permission bits.
    pub mode: u32,
}

impl Default for LeaseOptions {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_LEASE_ATTEMPTS,
            retry_max: DEFAULT_LEASE_RETRY_MAX,
            mode: DEFAULT_LEASE_MODE,
        }
    }
}

/// An exclusive lease on the lease file itself. Dropping it closes and unlocks it.
#[derive(Debug)]
pub struct FileLease {
    _guard: Flock<File>,
    path: PathBuf,
}

impl FileLease {
    /// The lease file protected by this guard.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Acquire an exclusive nonblocking lease with bounded jittered retries.
///
/// Unlike [`crate::hold_lock`], this locks `path` itself and returns a guard
/// meant to stay alive across work outside a short critical section.
pub fn acquire_file_lease(
    path: impl AsRef<Path>,
    options: LeaseOptions,
) -> Result<Option<FileLease>, LeaseError> {
    let path = path.as_ref();
    fs::create_dir_all(parent_dir(path)).map_err(|source| io_error(path, source))?;
    let attempts = options.attempts.max(1);
    let deadline = Instant::now()
        .checked_add(options.retry_max)
        .unwrap_or_else(Instant::now);

    for attempt in 0..attempts {
        let mut open_options = OpenOptions::new();
        open_options.read(true).write(true).create(true);
        #[cfg(unix)]
        open_options.mode(options.mode);
        let file = open_options
            .open(path)
            .map_err(|source| io_error(path, source))?;
        // `mode_t` is u32 on Linux and u16 on Apple targets, so the bitflags
        // constructor takes a different width per platform. Narrowing through
        // the libc type keeps one expression correct on both rather than
        // pinning whichever one the host happens to be.
        fchmod(
            &file,
            Mode::from_bits_truncate(options.mode as nix::libc::mode_t),
        )
        .map_err(|source| io_error(path, io::Error::from_raw_os_error(source as i32)))?;

        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(guard) => {
                return Ok(Some(FileLease {
                    _guard: guard,
                    path: path.to_path_buf(),
                }));
            }
            Err((file, error)) if is_contention(error) => {
                drop(file);
                if attempt == attempts - 1 || Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(retry_delay(deadline));
            }
            Err((file, error)) => {
                drop(file);
                return Err(io_error(path, io::Error::from_raw_os_error(error as i32)));
            }
        }
    }
    Ok(None)
}

fn is_contention(error: Errno) -> bool {
    error == Errno::EACCES || error == Errno::EAGAIN || error == Errno::EWOULDBLOCK
}

fn retry_delay(deadline: Instant) -> Duration {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let minimum = Duration::from_millis(10);
    let maximum = Duration::from_millis(250);
    let jitter = if minimum >= maximum {
        maximum
    } else {
        let span = maximum - minimum;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let entropy = now ^ u128::from(std::process::id());
        minimum + Duration::from_nanos((entropy % (span.as_nanos() + 1)) as u64)
    };
    remaining.min(jitter)
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn io_error(path: &Path, source: io::Error) -> LeaseError {
    LeaseError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn acquire_release_and_default_mode() {
        let temporary = TempDir::new();
        let path = temporary.path().join("health/refresh.lease");
        let lease = acquire_file_lease(&path, LeaseOptions::default())
            .unwrap()
            .expect("first acquire succeeds");
        assert_eq!(lease.path(), path);
        assert!(path.exists());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(lease);

        assert!(
            acquire_file_lease(&path, LeaseOptions::default())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn configured_mode_is_applied() {
        let temporary = TempDir::new();
        let path = temporary.path().join("refresh.lease");
        let lease = acquire_file_lease(
            &path,
            LeaseOptions {
                mode: 0o640,
                ..LeaseOptions::default()
            },
        )
        .unwrap()
        .expect("acquire succeeds");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        drop(lease);
    }

    #[test]
    fn lease_pause_helper() {
        let Ok(path) = std::env::var("JOURNAL_IO_HELPER_LEASE_PATH") else {
            return;
        };
        let lease = acquire_file_lease(path, LeaseOptions::default())
            .unwrap()
            .expect("helper acquires lease");
        let marker = std::env::var("JOURNAL_IO_TEST_MARKER").unwrap();
        fs::write(marker, "locked").unwrap();
        let _keep_guard_alive = lease;
        loop {
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn contention_returns_none_from_another_process() {
        let temporary = TempDir::new();
        let path = temporary.path().join("refresh.lease");
        let holder = spawn_lease_holder(&path, temporary.path());
        wait_for_marker(&holder.marker);

        let contention = acquire_file_lease(
            &path,
            LeaseOptions {
                attempts: 2,
                retry_max: Duration::from_millis(25),
                ..LeaseOptions::default()
            },
        );
        assert!(matches!(contention, Ok(None)));
        kill_holder(holder);
    }

    #[test]
    fn lease_is_released_when_the_holder_dies() {
        let temporary = TempDir::new();
        let path = temporary.path().join("refresh.lease");
        let mut holder = spawn_lease_holder(&path, temporary.path());
        wait_for_marker(&holder.marker);

        assert!(matches!(
            acquire_file_lease(
                &path,
                LeaseOptions {
                    retry_max: Duration::ZERO,
                    ..LeaseOptions::default()
                },
            ),
            Ok(None)
        ));
        kill(Pid::from_raw(holder.child.id() as i32), Signal::SIGKILL).unwrap();
        holder.child.wait().unwrap();
        fs::remove_file(&holder.marker).unwrap();
        fs::remove_file(holder.marker.with_extension("pid")).unwrap();

        let started = Instant::now();
        assert!(
            acquire_file_lease(&path, LeaseOptions::default())
                .unwrap()
                .is_some()
        );
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn zero_attempts_and_retry_window_make_one_immediate_attempt() {
        let temporary = TempDir::new();
        let path = temporary.path().join("refresh.lease");
        let holder = spawn_lease_holder(&path, temporary.path());
        wait_for_marker(&holder.marker);

        let started = Instant::now();
        let result = acquire_file_lease(
            &path,
            LeaseOptions {
                attempts: 0,
                retry_max: Duration::ZERO,
                ..LeaseOptions::default()
            },
        );
        assert!(matches!(result, Ok(None)));
        assert!(started.elapsed() < Duration::from_millis(100));
        kill_holder(holder);
    }

    struct LeaseHolder {
        child: std::process::Child,
        marker: PathBuf,
    }

    fn spawn_lease_holder(path: &Path, temporary: &Path) -> LeaseHolder {
        let marker = temporary.join(format!("lease-holder-{}.ready", std::process::id()));
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lease::tests::lease_pause_helper", "--nocapture"])
            .env("JOURNAL_IO_HELPER_LEASE_PATH", path)
            .env("JOURNAL_IO_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        fs::write(marker.with_extension("pid"), child.id().to_string()).unwrap();
        LeaseHolder { child, marker }
    }

    fn wait_for_marker(marker: &Path) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "helper did not acquire the lease");
    }

    fn kill_holder(mut holder: LeaseHolder) {
        kill(Pid::from_raw(holder.child.id() as i32), Signal::SIGKILL).unwrap();
        holder.child.wait().unwrap();
        fs::remove_file(&holder.marker).unwrap();
        fs::remove_file(holder.marker.with_extension("pid")).unwrap();
    }
}
