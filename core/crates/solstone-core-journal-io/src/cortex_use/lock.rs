// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persistent exclusive lock for Cortex's admitted journal namespace.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::time::Duration;

use super::namespace::CortexNamespaceAuthority;
use crate::errors::ExistingParentLockError;
use crate::locking::{
    BoundParentLock, DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT,
    acquire_existing_parent_lock_bound,
};

#[cfg(all(test, unix))]
use std::path::Path;
#[cfg(all(test, unix))]
use std::process::Command;

#[cfg(all(test, unix))]
use crate::journal_root::JournalRoot;

const CORTEX_NAMESPACE_LOCK_NAME: &str = "cortex-use.lock";

/// Retained exclusive lock for Cortex's admitted journal namespace.
///
/// Dropping this value releases the advisory lock but leaves its persistent
/// journal-root entry in place.
#[derive(Debug)]
pub struct CortexNamespaceLock {
    _guard: BoundParentLock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CortexNamespaceLockClass {
    Unsafe,
    IdentityChanged,
    Busy,
    Io,
}

impl CortexNamespaceLockClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::IdentityChanged => "identity_changed",
            Self::Busy => "busy",
            Self::Io => "io",
        }
    }
}

/// Bounded failure while acquiring Cortex's persistent namespace lock.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CortexNamespaceLockError {
    class: CortexNamespaceLockClass,
}

impl CortexNamespaceLockError {
    const fn new(class: CortexNamespaceLockClass) -> Self {
        Self { class }
    }

    fn token(self) -> String {
        format!("cortex_namespace_lock_{}", self.class.token())
    }
}

impl fmt::Display for CortexNamespaceLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for CortexNamespaceLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for CortexNamespaceLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Acquire Cortex's process-lifetime lock beneath its admitted journal root.
pub fn acquire_cortex_namespace_lock(
    authority: &CortexNamespaceAuthority,
) -> Result<CortexNamespaceLock, CortexNamespaceLockError> {
    acquire_cortex_namespace_lock_with_timeout(
        authority,
        DEFAULT_LOCK_TIMEOUT,
        DEFAULT_LOCK_POLL_INTERVAL,
    )
}

fn acquire_cortex_namespace_lock_with_timeout(
    authority: &CortexNamespaceAuthority,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<CortexNamespaceLock, CortexNamespaceLockError> {
    acquire_existing_parent_lock_bound(
        authority.root(),
        OsStr::new(CORTEX_NAMESPACE_LOCK_NAME),
        timeout,
        poll_interval,
    )
    .map(|guard| CortexNamespaceLock { _guard: guard })
    .map_err(map_existing_parent_lock_error)
}

fn map_existing_parent_lock_error(error: ExistingParentLockError) -> CortexNamespaceLockError {
    let class = match error {
        ExistingParentLockError::InvalidLockPath { .. }
        | ExistingParentLockError::MissingParent { .. }
        | ExistingParentLockError::UnsafeParent { .. }
        | ExistingParentLockError::UnsafeLockEntry { .. }
        | ExistingParentLockError::WrongMode { .. } => CortexNamespaceLockClass::Unsafe,
        ExistingParentLockError::ParentChanged { .. }
        | ExistingParentLockError::NamespaceChanged { .. } => {
            CortexNamespaceLockClass::IdentityChanged
        }
        ExistingParentLockError::Timeout(_) => CortexNamespaceLockClass::Busy,
        ExistingParentLockError::Io { .. } => CortexNamespaceLockClass::Io,
    };
    CortexNamespaceLockError::new(class)
}

#[cfg(all(test, unix))]
fn create_cortex_namespace_inert_socket(path: &Path) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    nix::sys::stat::mknod(
        path,
        nix::sys::stat::SFlag::S_IFSOCK,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        0,
    )
    .unwrap();
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    drop(std::os::unix::net::UnixListener::bind(path).unwrap());
}

#[cfg(all(test, unix))]
fn verify_cortex_namespace_lock_cross_process(root: &Path) {
    let marker = root.join("cortex-namespace-lock.ready");
    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "cortex_use::lock::tests::cortex_namespace_lock_pause_helper",
            "--nocapture",
        ])
        .env("JOURNAL_IO_CORTEX_NAMESPACE_LOCK_ROOT", root)
        .env("JOURNAL_IO_CORTEX_NAMESPACE_LOCK_READY", &marker)
        .spawn()
        .expect("run namespace-lock holder");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "helper did not acquire the namespace lock");

    let authority =
        super::namespace::create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap())
            .unwrap();
    let error =
        acquire_cortex_namespace_lock_with_timeout(&authority, Duration::ZERO, Duration::ZERO)
            .unwrap_err();
    assert_eq!(error.to_string(), "cortex_namespace_lock_busy");
    child.kill().unwrap();
    child.wait().unwrap();
    acquire_cortex_namespace_lock_with_timeout(&authority, Duration::ZERO, Duration::ZERO).unwrap();
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    use super::*;
    use crate::errors::LockTimeout;
    use crate::journal_root::JournalRoot;

    const LOCK_ROOT_ENV: &str = "JOURNAL_IO_CORTEX_NAMESPACE_LOCK_ROOT";
    const LOCK_READY_ENV: &str = "JOURNAL_IO_CORTEX_NAMESPACE_LOCK_READY";

    fn authority(root: &Path) -> CortexNamespaceAuthority {
        super::super::namespace::create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap())
            .unwrap()
    }

    fn entry_mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    fn entry_identity(path: &Path) -> (u64, u64, fs::FileType) {
        let metadata = fs::symlink_metadata(path).unwrap();
        (metadata.dev(), metadata.ino(), metadata.file_type())
    }

    fn expect_busy(error: CortexNamespaceLockError) {
        assert_eq!(error.to_string(), "cortex_namespace_lock_busy");
        assert_eq!(format!("{error:?}"), "cortex_namespace_lock_busy");
        assert!(error.source().is_none());
    }

    #[test]
    fn mapper_is_exhaustive_and_bounded() {
        let sentinel = PathBuf::from("controlled-secret");
        for (error, expected) in [
            (
                ExistingParentLockError::InvalidLockPath {
                    name: OsString::from("controlled-secret"),
                },
                "unsafe",
            ),
            (
                ExistingParentLockError::MissingParent {
                    parent: sentinel.clone(),
                },
                "unsafe",
            ),
            (
                ExistingParentLockError::UnsafeParent {
                    parent: sentinel.clone(),
                    kind: "controlled-secret",
                },
                "unsafe",
            ),
            (
                ExistingParentLockError::UnsafeLockEntry {
                    path: sentinel.clone(),
                    kind: "controlled-secret",
                },
                "unsafe",
            ),
            (
                ExistingParentLockError::WrongMode {
                    path: sentinel.clone(),
                    observed: 0o644,
                },
                "unsafe",
            ),
            (
                ExistingParentLockError::ParentChanged {
                    parent: sentinel.clone(),
                },
                "identity_changed",
            ),
            (
                ExistingParentLockError::NamespaceChanged {
                    path: sentinel.clone(),
                },
                "identity_changed",
            ),
            (
                ExistingParentLockError::Timeout(LockTimeout {
                    path: sentinel.clone(),
                    timeout: Duration::from_secs(3),
                }),
                "busy",
            ),
            (
                ExistingParentLockError::Io {
                    operation: "controlled-secret",
                    path: sentinel,
                    source: io::Error::other("controlled-secret"),
                },
                "io",
            ),
        ] {
            let mapped = map_existing_parent_lock_error(error);
            let expected = format!("cortex_namespace_lock_{expected}");
            assert_eq!(mapped.to_string(), expected);
            assert_eq!(format!("{mapped:?}"), expected);
            assert!(mapped.source().is_none());
        }
    }

    #[test]
    fn mapper_does_not_leak_failure_details() {
        let sentinel = "fixture-sentinel-must-not-leak";
        for error in [
            ExistingParentLockError::UnsafeLockEntry {
                path: PathBuf::from(sentinel),
                kind: sentinel,
            },
            ExistingParentLockError::Io {
                operation: sentinel,
                path: PathBuf::from(sentinel),
                source: io::Error::other(sentinel),
            },
        ] {
            let rendered = map_existing_parent_lock_error(error).to_string();
            assert!(!rendered.contains(sentinel));
            assert!(!format!("{rendered:?}").contains(sentinel));
        }
    }

    #[test]
    fn same_process_exclusion_persists_and_reacquires() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let authority = authority(temporary.path());
        let first =
            acquire_cortex_namespace_lock_with_timeout(&authority, Duration::ZERO, Duration::ZERO)
                .unwrap();
        let entry = temporary.path().join(CORTEX_NAMESPACE_LOCK_NAME);
        let identity = entry_identity(&entry);
        assert_eq!(entry_mode(&entry), 0o600);
        expect_busy(
            acquire_cortex_namespace_lock_with_timeout(&authority, Duration::ZERO, Duration::ZERO)
                .unwrap_err(),
        );
        drop(first);

        let second =
            acquire_cortex_namespace_lock_with_timeout(&authority, Duration::ZERO, Duration::ZERO)
                .unwrap();
        assert_eq!(entry_identity(&entry), identity);
        assert_eq!(entry_mode(&entry), 0o600);
        drop(second);
        assert!(entry.exists());
    }

    #[test]
    fn root_lock_straddles_health_replacement_between_authorities() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path();
        let authority_a = authority(root);
        let lock_a = acquire_cortex_namespace_lock_with_timeout(
            &authority_a,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        fs::rename(root.join("health"), root.join("health-moved")).unwrap();
        fs::create_dir(root.join("health")).unwrap();

        let authority_b = authority(root);
        expect_busy(
            acquire_cortex_namespace_lock_with_timeout(
                &authority_b,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap_err(),
        );
        assert!(root.join("health-moved").is_dir());
        assert!(root.join("health").is_dir());
        drop(lock_a);
    }

    #[test]
    fn unsafe_entries_are_refused_unchanged_and_valid_entry_is_reused() {
        for kind in ["symlink", "wrong-mode", "directory", "fifo", "socket"] {
            let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
            let root = temporary.path();
            let authority = authority(root);
            let entry = root.join(CORTEX_NAMESPACE_LOCK_NAME);
            match kind {
                "symlink" => {
                    fs::write(root.join("outside"), b"outside").unwrap();
                    symlink("outside", &entry).unwrap();
                }
                "wrong-mode" => {
                    fs::write(&entry, b"unchanged").unwrap();
                    fs::set_permissions(&entry, fs::Permissions::from_mode(0o644)).unwrap();
                }
                "directory" => fs::create_dir(&entry).unwrap(),
                "fifo" => mkfifo(&entry, Mode::S_IRUSR | Mode::S_IWUSR).unwrap(),
                "socket" => create_cortex_namespace_inert_socket(&entry),
                _ => unreachable!(),
            }
            let identity = entry_identity(&entry);
            let bytes = (kind == "wrong-mode").then(|| fs::read(&entry).unwrap());

            let error = acquire_cortex_namespace_lock_with_timeout(
                &authority,
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap_err();

            assert_eq!(error.to_string(), "cortex_namespace_lock_unsafe");
            assert_eq!(entry_identity(&entry), identity);
            assert_eq!(
                (kind == "wrong-mode").then(|| fs::read(&entry).unwrap()),
                bytes
            );
        }

        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path();
        let authority = authority(root);
        let entry = root.join(CORTEX_NAMESPACE_LOCK_NAME);
        fs::File::create(&entry).unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
        let identity = entry_identity(&entry);

        let lock =
            acquire_cortex_namespace_lock_with_timeout(&authority, Duration::ZERO, Duration::ZERO)
                .unwrap();
        assert_eq!(entry_identity(&entry), identity);
        assert_eq!(entry_mode(&entry), 0o600);
        drop(lock);
    }

    #[test]
    fn cortex_namespace_lock_pause_helper() {
        let Some(root) = std::env::var_os(LOCK_ROOT_ENV) else {
            return;
        };
        let marker = PathBuf::from(std::env::var_os(LOCK_READY_ENV).unwrap());
        let authority = authority(Path::new(&root));
        let _lock = acquire_cortex_namespace_lock(&authority).unwrap();
        fs::write(marker, "locked").unwrap();
        loop {
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    #[ignore = "VPE-direct multi-process verification"]
    fn cross_process_holder_is_busy_then_releases_after_death() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        verify_cortex_namespace_lock_cross_process(temporary.path());
    }
}
