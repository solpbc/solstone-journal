// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Advisory file leases held for the lifetime of their guard.

#[cfg(unix)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{FcntlArg, Flock, FlockArg, fcntl};
#[cfg(unix)]
use nix::sys::stat::{Mode, fchmod};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT, OPEN_ALWAYS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::errors::LeaseError;
#[cfg(windows)]
use crate::windows_lock::{
    WindowsLockGuard, WindowsLockHeld, is_contention as windows_contention, try_lock_exclusive,
    try_lock_exclusive_held,
};

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
    /// Lease-file permission bits. Windows does not apply this field; it has no ACL equivalent.
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
    #[cfg(unix)]
    _guard: Flock<File>,
    #[cfg(windows)]
    _guard: WindowsLockGuard,
    path: PathBuf,
}

/// Exclusive lease taken on an already-open file's own descriptor.
///
/// Dropping this value closes the lease descriptor. It does not close the
/// caller's original `File`. The advisory lock is released when the last
/// descriptor of this open file description (Unix) or locked handle (Windows)
/// closes; there is no explicit unlock on drop.
#[derive(Debug)]
pub struct SelfLease {
    #[cfg(unix)]
    #[allow(dead_code)]
    file: File,
    #[cfg(windows)]
    _held: WindowsLockHeld,
}

/// Result of a non-blocking exclusive try-lock used as a liveness probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseProbe {
    /// An exclusive lock is currently held.
    Active,
    /// An exclusive lock was acquired and released immediately.
    Released,
    /// The probe could not classify the lease.
    Indeterminate,
}

impl FileLease {
    /// The lease file protected by this guard.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Duplicate this lease descriptor for inheritance across an `exec`.
    ///
    /// Callers pair the inherited descriptor with their own authenticated token
    /// in the lease file and close the returned descriptor when their process
    /// tree no longer needs it.
    #[cfg(unix)]
    pub fn duplicate_for_inheritance(&self) -> Result<i32, LeaseError> {
        fcntl(&*self._guard, FcntlArg::F_DUPFD(3))
            .map_err(|error| io_error(&self.path, io::Error::from_raw_os_error(error as i32)))
    }
}

/// Acquire an exclusive nonblocking lease with bounded jittered retries.
///
/// Unlike [`crate::hold_lock`], this locks `path` itself and returns a guard
/// meant to stay alive across work outside a short critical section.
#[cfg(unix)]
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

/// Take an exclusive nonblocking lease on `file` itself, without a companion path.
pub fn acquire_self_lease(file: &File) -> Result<Option<SelfLease>, LeaseError> {
    #[cfg(unix)]
    {
        acquire_self_lease_unix(file)
    }
    #[cfg(windows)]
    {
        acquire_self_lease_windows(file)
    }
}

/// Non-blocking exclusive try-lock used as a liveness probe on an open handle.
pub fn probe_file_lease(file: &File) -> LeaseProbe {
    match acquire_self_lease(file) {
        Ok(Some(lease)) => {
            drop(lease);
            LeaseProbe::Released
        }
        Ok(None) => LeaseProbe::Active,
        Err(_) => LeaseProbe::Indeterminate,
    }
}

#[cfg(unix)]
fn acquire_self_lease_unix(file: &File) -> Result<Option<SelfLease>, LeaseError> {
    let duplicated = fcntl(file, FcntlArg::F_DUPFD_CLOEXEC(3)).map_err(|error| {
        io_error(
            Path::new("self-lease"),
            io::Error::from_raw_os_error(error as i32),
        )
    })?;
    // SAFETY: `duplicated` is a freshly allocated descriptor owned by this function.
    #[allow(unsafe_code)]
    let owned = unsafe { OwnedFd::from_raw_fd(duplicated) };
    let file = File::from(owned);
    // SAFETY: `file` owns a live descriptor; `LOCK_NB` makes this call non-blocking.
    #[allow(unsafe_code)]
    let result =
        unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    if result == 0 {
        Ok(Some(SelfLease { file }))
    } else {
        let error = Errno::last();
        drop(file);
        if is_contention(error) {
            Ok(None)
        } else {
            Err(io_error(
                Path::new("self-lease"),
                io::Error::from_raw_os_error(error as i32),
            ))
        }
    }
}

#[cfg(windows)]
fn acquire_self_lease_windows(file: &File) -> Result<Option<SelfLease>, LeaseError> {
    let duplicated = duplicate_windows_handle(file)?;
    match try_lock_exclusive_held(duplicated) {
        Ok(held) => Ok(Some(SelfLease { _held: held })),
        Err((file, error)) if windows_contention(&error) => {
            drop(file);
            Ok(None)
        }
        Err((file, error)) => {
            drop(file);
            Err(io_error(Path::new("self-lease"), error))
        }
    }
}

#[cfg(windows)]
fn duplicate_windows_handle(file: &File) -> Result<File, LeaseError> {
    let mut duplicated = INVALID_HANDLE_VALUE;
    // SAFETY: source handle is a live `File`; output pointer is a local HANDLE.
    #[allow(unsafe_code)]
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicated,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if result == 0 || duplicated == INVALID_HANDLE_VALUE {
        return Err(io_error(
            Path::new("self-lease"),
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: `duplicated` is an owned handle returned by DuplicateHandle.
    #[allow(unsafe_code)]
    Ok(File::from(unsafe {
        OwnedHandle::from_raw_handle(duplicated)
    }))
}

#[cfg(unix)]
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

#[cfg(windows)]
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
        let file = crate::locking::open_windows_path(
            path,
            GENERIC_READ | GENERIC_WRITE,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .map_err(|source| io_error(path, source))?;
        match try_lock_exclusive(file) {
            Ok(guard) => {
                return Ok(Some(FileLease {
                    _guard: guard,
                    path: path.to_path_buf(),
                }));
            }
            Err((file, error)) if windows_contention(&error) => {
                drop(file);
                if attempt == attempts - 1 || Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(retry_delay(deadline));
            }
            Err((file, error)) => {
                drop(file);
                return Err(io_error(path, error));
            }
        }
    }
    Ok(None)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

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

    #[cfg(unix)]
    #[test]
    fn inherited_descriptor_is_a_live_duplicate() {
        let temporary = TempDir::new();
        let lease = acquire_file_lease(
            temporary.path().join("generation.lock"),
            LeaseOptions::default(),
        )
        .unwrap()
        .expect("lease");
        let descriptor = lease
            .duplicate_for_inheritance()
            .expect("duplicate descriptor");
        assert!(Path::new("/dev/fd").join(descriptor.to_string()).exists());
        nix::unistd::close(descriptor).expect("close duplicate");
    }
}
