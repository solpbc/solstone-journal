// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persistent exclusive lock for Cortex's admitted journal namespace.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
#[cfg(any(test, feature = "test-hooks"))]
use std::time::Duration;

use super::namespace::CortexNamespaceAuthority;
use crate::errors::ExistingParentLockError;
use crate::locking::{
    BoundParentLock, DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT,
    acquire_existing_parent_lock_bound,
};
#[cfg(all(test, unix))]
use std::path::Path;

const CORTEX_NAMESPACE_LOCK_NAME: &str = "cortex-use.lock";

/// Retained exclusive lock for Cortex's admitted journal namespace.
///
/// Dropping this value releases the advisory lock but leaves its persistent
/// journal-root entry in place.
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
    acquire_existing_parent_lock_bound(
        authority.root(),
        OsStr::new(CORTEX_NAMESPACE_LOCK_NAME),
        DEFAULT_LOCK_TIMEOUT,
        DEFAULT_LOCK_POLL_INTERVAL,
    )
    .map(|guard| CortexNamespaceLock { _guard: guard })
    .map_err(map_existing_parent_lock_error)
}

#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn acquire_cortex_namespace_lock_with_test_timing(
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
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    use super::*;
    use crate::errors::LockTimeout;
    use crate::journal_root::JournalRoot;

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

    fn expect_lock_error(
        result: Result<CortexNamespaceLock, CortexNamespaceLockError>,
    ) -> CortexNamespaceLockError {
        match result {
            Ok(_) => panic!("expected Cortex namespace lock acquisition to fail"),
            Err(error) => error,
        }
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
        let first = acquire_cortex_namespace_lock_with_test_timing(
            &authority,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        let entry = temporary.path().join(CORTEX_NAMESPACE_LOCK_NAME);
        let identity = entry_identity(&entry);
        assert_eq!(entry_mode(&entry), 0o600);
        assert!(fs::read(&entry).unwrap().is_empty());
        expect_busy(expect_lock_error(
            acquire_cortex_namespace_lock_with_test_timing(
                &authority,
                Duration::ZERO,
                Duration::ZERO,
            ),
        ));
        drop(first);

        let second = acquire_cortex_namespace_lock_with_test_timing(
            &authority,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(entry_identity(&entry), identity);
        assert_eq!(entry_mode(&entry), 0o600);
        assert!(fs::read(&entry).unwrap().is_empty());
        drop(second);
        assert!(entry.exists());
    }

    #[test]
    fn root_lock_straddles_health_replacement_between_authorities() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path();
        fs::write(root.join("root-sentinel"), b"root").unwrap();
        let authority_a = authority(root);
        fs::write(root.join("health/sentinel"), b"old-health").unwrap();
        fs::write(root.join("talents/sentinel"), b"talents").unwrap();
        let root_identity = authority_a.root().identity();
        let old_health_identity = authority_a.health().identity();
        let talents_identity = authority_a.talents().identity();
        let lock_a = acquire_cortex_namespace_lock_with_test_timing(
            &authority_a,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        let lock_entry = root.join(CORTEX_NAMESPACE_LOCK_NAME);
        let lock_identity = entry_identity(&lock_entry);
        fs::rename(root.join("health"), root.join("health-moved")).unwrap();
        fs::create_dir(root.join("health")).unwrap();
        fs::write(root.join("health/sentinel"), b"replacement-health").unwrap();

        let authority_b = authority(root);
        assert_eq!(authority_a.root().identity(), root_identity);
        assert_eq!(authority_b.root().identity(), root_identity);
        assert_eq!(authority_a.talents().identity(), talents_identity);
        assert_eq!(authority_b.talents().identity(), talents_identity);
        assert_eq!(authority_a.health().identity(), old_health_identity);
        assert_ne!(authority_b.health().identity(), old_health_identity);
        expect_busy(expect_lock_error(
            acquire_cortex_namespace_lock_with_test_timing(
                &authority_b,
                Duration::ZERO,
                Duration::ZERO,
            ),
        ));
        assert_eq!(entry_identity(&lock_entry), lock_identity);
        assert_eq!(fs::read(root.join("root-sentinel")).unwrap(), b"root");
        assert_eq!(
            fs::read(root.join("health-moved/sentinel")).unwrap(),
            b"old-health"
        );
        assert_eq!(
            fs::read(root.join("health/sentinel")).unwrap(),
            b"replacement-health"
        );
        assert_eq!(fs::read(root.join("talents/sentinel")).unwrap(), b"talents");
        assert!(!root.join("health-moved/cortex-use.lock").exists());
        assert!(!root.join("health/cortex-use.lock").exists());
        authority_a.root().revalidate().unwrap();
        authority_a.health().revalidate().unwrap();
        authority_a.talents().revalidate().unwrap();
        authority_b.root().revalidate().unwrap();
        authority_b.health().revalidate().unwrap();
        authority_b.talents().revalidate().unwrap();
        drop(lock_a);

        let lock_b = acquire_cortex_namespace_lock_with_test_timing(
            &authority_b,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(entry_identity(&lock_entry), lock_identity);
        drop(lock_b);
    }

    #[test]
    fn unsafe_entries_are_refused_unchanged_and_valid_entry_is_reused() {
        for kind in ["symlink", "wrong-mode", "directory", "fifo", "socket"] {
            let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
            let root = temporary.path();
            let authority = authority(root);
            fs::write(root.join("root-sentinel"), b"root").unwrap();
            fs::write(root.join("health/sentinel"), b"health").unwrap();
            fs::write(root.join("talents/sentinel"), b"talents").unwrap();
            let root_identity = authority.root().identity();
            let health_identity = authority.health().identity();
            let talents_identity = authority.talents().identity();
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
            let mode = entry_mode(&entry);
            let bytes = (kind == "wrong-mode").then(|| fs::read(&entry).unwrap());
            let outside = (kind == "symlink").then(|| {
                let path = root.join("outside");
                (entry_identity(&path), fs::read(&path).unwrap())
            });

            let error = expect_lock_error(acquire_cortex_namespace_lock_with_test_timing(
                &authority,
                Duration::ZERO,
                Duration::ZERO,
            ));

            assert_eq!(error.to_string(), "cortex_namespace_lock_unsafe");
            assert_eq!(entry_identity(&entry), identity);
            assert_eq!(entry_mode(&entry), mode);
            assert_eq!(
                (kind == "wrong-mode").then(|| fs::read(&entry).unwrap()),
                bytes
            );
            assert_eq!(
                (kind == "symlink").then(|| {
                    let path = root.join("outside");
                    (entry_identity(&path), fs::read(&path).unwrap())
                }),
                outside
            );
            assert_eq!(authority.root().identity(), root_identity);
            assert_eq!(authority.health().identity(), health_identity);
            assert_eq!(authority.talents().identity(), talents_identity);
            authority.root().revalidate().unwrap();
            authority.health().revalidate().unwrap();
            authority.talents().revalidate().unwrap();
            assert_eq!(fs::read(root.join("root-sentinel")).unwrap(), b"root");
            assert_eq!(fs::read(root.join("health/sentinel")).unwrap(), b"health");
            assert_eq!(fs::read(root.join("talents/sentinel")).unwrap(), b"talents");
        }

        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path();
        let single_authority = authority(root);
        let entry = root.join(CORTEX_NAMESPACE_LOCK_NAME);
        fs::write(&entry, b"valid-entry-bytes").unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
        let identity = entry_identity(&entry);

        let lock = acquire_cortex_namespace_lock_with_test_timing(
            &single_authority,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(entry_identity(&entry), identity);
        assert_eq!(entry_mode(&entry), 0o600);
        assert_eq!(fs::read(&entry).unwrap(), b"valid-entry-bytes");
        drop(lock);

        let left = tempfile::tempdir_in("/var/tmp").unwrap();
        let right = tempfile::tempdir_in("/var/tmp").unwrap();
        let left_authority = authority(left.path());
        let right_authority = authority(right.path());
        let left_entry = left.path().join(CORTEX_NAMESPACE_LOCK_NAME);
        let right_entry = right.path().join(CORTEX_NAMESPACE_LOCK_NAME);
        for path in [&left_entry, &right_entry] {
            fs::write(path, b"byte-identical-valid-entry").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let left_identity = entry_identity(&left_entry);
        let right_identity = entry_identity(&right_entry);
        assert_ne!(left_identity, right_identity);

        let left_lock = acquire_cortex_namespace_lock_with_test_timing(
            &left_authority,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        let right_lock = acquire_cortex_namespace_lock_with_test_timing(
            &right_authority,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(entry_identity(&left_entry), left_identity);
        assert_eq!(entry_identity(&right_entry), right_identity);
        assert_eq!(
            fs::read(&left_entry).unwrap(),
            b"byte-identical-valid-entry"
        );
        assert_eq!(
            fs::read(&right_entry).unwrap(),
            b"byte-identical-valid-entry"
        );
        drop(right_lock);
        drop(left_lock);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::fs;
    use std::os::windows::fs::{MetadataExt, symlink_file};
    use std::path::Path;
    use std::time::Duration;

    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    use super::*;
    use crate::journal_root::JournalRoot;
    use crate::locking::{
        with_windows_lock_entry_observation_fault, with_windows_wrong_kind_replacement_hook,
    };

    fn authority(root: &Path) -> CortexNamespaceAuthority {
        super::super::namespace::create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap())
            .unwrap()
    }

    fn assert_file_reparse(path: &Path) {
        assert_ne!(
            fs::symlink_metadata(path).unwrap().file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
            0
        );
    }

    #[test]
    fn successful_reparse_open_replacement_maps_to_identity_changed() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path();
        let authority = authority(root);
        let entry = root.join(CORTEX_NAMESPACE_LOCK_NAME);
        let displaced = root.join("displaced-link");
        let replacement = root.join("replacement-link");
        let old_target = root.join("old-target");
        let new_target = root.join("new-target");
        fs::write(&old_target, b"old").unwrap();
        fs::write(&new_target, b"new").unwrap();
        symlink_file(&old_target, &entry).unwrap();
        symlink_file(&new_target, &replacement).unwrap();

        let (result, consumed) = with_windows_wrong_kind_replacement_hook(
            entry.clone(),
            displaced.clone(),
            replacement,
            || acquire_cortex_namespace_lock(&authority),
        );
        assert!(consumed);
        assert_eq!(
            result.err().unwrap().class,
            CortexNamespaceLockClass::IdentityChanged
        );
        assert_file_reparse(&displaced);
        assert_file_reparse(&entry);
        assert_eq!(fs::read(&old_target).unwrap(), b"old");
        assert_eq!(fs::read(&new_target).unwrap(), b"new");
    }

    #[test]
    fn successful_reparse_open_classification_failures_map_to_io() {
        for ordinal in [2, 3] {
            let temporary = tempfile::TempDir::new().unwrap();
            let root = temporary.path();
            let authority = authority(root);
            let target = root.join("target");
            let entry = root.join(CORTEX_NAMESPACE_LOCK_NAME);
            fs::write(&target, b"target").unwrap();
            symlink_file(&target, &entry).unwrap();

            let (result, consumed) = with_windows_lock_entry_observation_fault(ordinal, || {
                acquire_cortex_namespace_lock_with_test_timing(
                    &authority,
                    Duration::ZERO,
                    Duration::ZERO,
                )
            });
            assert!(consumed, "classification fault ordinal {ordinal}");
            assert_eq!(result.err().unwrap().class, CortexNamespaceLockClass::Io);
            assert_file_reparse(&entry);
            assert_eq!(fs::read(&target).unwrap(), b"target");
        }
    }
}
