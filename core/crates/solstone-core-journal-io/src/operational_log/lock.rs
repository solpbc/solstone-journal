// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persistent exclusive lock for one day-health operational-log namespace.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
#[cfg(any(test, feature = "test-hooks"))]
use std::time::Duration;

use super::namespace::OplogDayHealth;
use super::reason::OplogCreateLockClass;
use crate::errors::ExistingParentLockError;
use crate::locking::{
    BoundParentLock, DEFAULT_LOCK_POLL_INTERVAL, DEFAULT_LOCK_TIMEOUT,
    acquire_existing_parent_lock_bound,
};

const OPLOG_NAMESPACE_LOCK_NAME: &str = ".oplog-namespace.lock";

/// Retained exclusive lock for one day-health oplog namespace.
pub struct OplogNamespaceLock {
    _guard: BoundParentLock,
}

impl fmt::Debug for OplogNamespaceLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OplogNamespaceLock")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogNamespaceLockClass {
    Unsafe,
    IdentityChanged,
    Busy,
    Io,
}

impl OplogNamespaceLockClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::IdentityChanged => "identity_changed",
            Self::Busy => "busy",
            Self::Io => "io",
        }
    }
}

/// Bounded failure while acquiring the day-health oplog namespace lock.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogNamespaceLockError {
    class: OplogNamespaceLockClass,
}

impl OplogNamespaceLockError {
    const fn new(class: OplogNamespaceLockClass) -> Self {
        Self { class }
    }

    pub(super) const fn create_class(self) -> OplogCreateLockClass {
        match self.class {
            OplogNamespaceLockClass::Unsafe => OplogCreateLockClass::Unsafe,
            OplogNamespaceLockClass::IdentityChanged => OplogCreateLockClass::IdentityChanged,
            OplogNamespaceLockClass::Busy => OplogCreateLockClass::Busy,
            OplogNamespaceLockClass::Io => OplogCreateLockClass::Io,
        }
    }

    fn token(self) -> String {
        format!("oplog_namespace_lock_{}", self.class.token())
    }
}

impl fmt::Display for OplogNamespaceLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for OplogNamespaceLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogNamespaceLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Acquire the persistent lock beneath the admitted day-health directory.
pub fn acquire_oplog_namespace_lock(
    health: &OplogDayHealth,
) -> Result<OplogNamespaceLock, OplogNamespaceLockError> {
    acquire_with_timing(health, DEFAULT_LOCK_TIMEOUT, DEFAULT_LOCK_POLL_INTERVAL)
}

/// Acquire the lock with caller-supplied timing.
#[cfg(any(test, feature = "test-hooks"))]
pub fn acquire_oplog_namespace_lock_with_test_timing(
    health: &OplogDayHealth,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<OplogNamespaceLock, OplogNamespaceLockError> {
    acquire_with_timing(health, timeout, poll_interval)
}

fn acquire_with_timing(
    health: &OplogDayHealth,
    timeout: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<OplogNamespaceLock, OplogNamespaceLockError> {
    acquire_existing_parent_lock_bound(
        health.health(),
        OsStr::new(OPLOG_NAMESPACE_LOCK_NAME),
        timeout,
        poll_interval,
    )
    .map(|guard| OplogNamespaceLock { _guard: guard })
    .map_err(map_existing_parent_lock_error)
}

fn map_existing_parent_lock_error(error: ExistingParentLockError) -> OplogNamespaceLockError {
    let class = match error {
        ExistingParentLockError::InvalidLockPath { .. }
        | ExistingParentLockError::MissingParent { .. }
        | ExistingParentLockError::UnsafeParent { .. }
        | ExistingParentLockError::UnsafeLockEntry { .. }
        | ExistingParentLockError::WrongMode { .. } => OplogNamespaceLockClass::Unsafe,
        ExistingParentLockError::ParentChanged { .. }
        | ExistingParentLockError::NamespaceChanged { .. } => {
            OplogNamespaceLockClass::IdentityChanged
        }
        ExistingParentLockError::Timeout(_) => OplogNamespaceLockClass::Busy,
        ExistingParentLockError::Io { .. } => OplogNamespaceLockClass::Io,
    };
    OplogNamespaceLockError::new(class)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ffi::OsString;
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::errors::LockTimeout;

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
            let expected = format!("oplog_namespace_lock_{expected}");
            assert_eq!(mapped.to_string(), expected);
            assert_eq!(format!("{mapped:?}"), expected);
            assert!(mapped.source().is_none());
        }
    }
}
