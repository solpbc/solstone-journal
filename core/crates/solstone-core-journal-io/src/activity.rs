// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cooperative journal activity exclusion. A lease is not writer settlement,
//! recovery coverage or authority to retire its persistent entry.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::errors::ExistingParentLockError;
use crate::journal_root::{JournalRoot, JournalRootError};
use crate::locking::{BoundParentLock, acquire_existing_parent_lock_bound};

const ENTRY: &str = "backup-activity.lock";

#[derive(Debug)]
pub enum ActivityUnavailable {
    Root(JournalRootError),
    Entry(ExistingParentLockError),
    Poisoned,
}

#[derive(Debug)]
pub enum ActivityAdmissionError {
    Busy,
    Unavailable(ActivityUnavailable),
}

impl fmt::Display for ActivityAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("journal activity admission is busy"),
            Self::Unavailable(ActivityUnavailable::Root(error)) => error.fmt(f),
            Self::Unavailable(ActivityUnavailable::Entry(error)) => error.fmt(f),
            Self::Unavailable(ActivityUnavailable::Poisoned) => {
                f.write_str("journal activity ownership is unavailable")
            }
        }
    }
}

impl Error for ActivityAdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unavailable(ActivityUnavailable::Root(error)) => Some(error),
            Self::Unavailable(ActivityUnavailable::Entry(error)) => Some(error),
            Self::Busy | Self::Unavailable(ActivityUnavailable::Poisoned) => None,
        }
    }
}

fn entry_error(error: ExistingParentLockError) -> ActivityAdmissionError {
    match error {
        ExistingParentLockError::Timeout(_) => ActivityAdmissionError::Busy,
        error => ActivityAdmissionError::Unavailable(ActivityUnavailable::Entry(error)),
    }
}

impl ActivityAdmissionError {
    pub(crate) fn into_io(self) -> std::io::Error {
        let kind = match &self {
            Self::Busy => std::io::ErrorKind::WouldBlock,
            Self::Unavailable(ActivityUnavailable::Entry(ExistingParentLockError::Io {
                source,
                ..
            }))
            | Self::Unavailable(ActivityUnavailable::Root(JournalRootError::Io {
                source, ..
            })) => source.kind(),
            Self::Unavailable(ActivityUnavailable::Root(JournalRootError::Unsupported {
                ..
            })) => std::io::ErrorKind::Unsupported,
            Self::Unavailable(ActivityUnavailable::Poisoned) => std::io::ErrorKind::Other,
            _ => std::io::ErrorKind::InvalidData,
        };
        std::io::Error::new(kind, self)
    }
}

struct Admission {
    root: Arc<JournalRoot>,
    // WindowsLockGuard is Send, not Sync. Only local validation borrows this
    // mutex; the actual native lock is retained for the whole ownership lifetime.
    guard: Mutex<BoundParentLock>,
}

impl Admission {
    fn acquire(root: Arc<JournalRoot>) -> Result<Self, ActivityAdmissionError> {
        root.revalidate_canonical_binding()
            .map_err(|e| ActivityAdmissionError::Unavailable(ActivityUnavailable::Root(e)))?;
        let guard = acquire_existing_parent_lock_bound(
            root.as_ref(),
            OsStr::new(ENTRY),
            Duration::ZERO,
            Duration::ZERO,
        )
        .map_err(entry_error)?;
        let admitted = Self {
            root,
            guard: Mutex::new(guard),
        };
        admitted.revalidate()?;
        Ok(admitted)
    }

    fn revalidate(&self) -> Result<(), ActivityAdmissionError> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| ActivityAdmissionError::Unavailable(ActivityUnavailable::Root(e)))?;
        self.guard
            .lock()
            .map_err(|_| ActivityAdmissionError::Unavailable(ActivityUnavailable::Poisoned))?
            .revalidate_bound(self.root.as_ref(), OsStr::new(ENTRY))
            .map_err(entry_error)?;
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| ActivityAdmissionError::Unavailable(ActivityUnavailable::Root(e)))
    }
}

/// Original admission for one cooperating activity and all its owned resources.
/// Cloning its Arc does not acquire another native lock or renew any budget.
pub struct ActivityLease {
    inner: Admission,
}

#[cfg(target_os = "linux")]
pub(crate) struct ExistingActivityFailure {
    pub(crate) lease: Option<Arc<ActivityLease>>,
    pub(crate) error: ActivityAdmissionError,
}

impl ActivityLease {
    /// Acquire only the producer's existing exclusion entry. Absence is not
    /// permission to create one while observing interrupted activity.
    pub fn open_existing(
        root: Arc<JournalRoot>,
    ) -> Result<Option<Arc<Self>>, ActivityAdmissionError> {
        root.revalidate_canonical_binding().map_err(|error| {
            ActivityAdmissionError::Unavailable(ActivityUnavailable::Root(error))
        })?;
        let guard = crate::locking::open_existing_parent_lock_bound(
            root.as_ref(),
            OsStr::new(ENTRY),
            Duration::ZERO,
            Duration::ZERO,
        )
        .map_err(entry_error)?;
        let Some(guard) = guard else {
            return Ok(None);
        };
        let lease = Arc::new(Self {
            inner: Admission {
                root,
                guard: Mutex::new(guard),
            },
        });
        lease.revalidate()?;
        Ok(Some(lease))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn try_acquire_existing(
        root: Arc<JournalRoot>,
    ) -> Result<Arc<Self>, ExistingActivityFailure> {
        Self::acquire_existing_with(root, |_| {})
    }

    #[cfg(target_os = "linux")]
    fn acquire_existing_with(
        root: Arc<JournalRoot>,
        after_acquisition: impl FnOnce(&ActivityLease),
    ) -> Result<Arc<Self>, ExistingActivityFailure> {
        let failure = |error| ExistingActivityFailure { lease: None, error };
        root.revalidate_canonical_binding().map_err(|error| {
            failure(ActivityAdmissionError::Unavailable(
                ActivityUnavailable::Root(error),
            ))
        })?;
        let guard = match crate::locking::open_existing_parent_lock_retained(
            root.as_ref(),
            OsStr::new(ENTRY),
            Duration::ZERO,
            Duration::ZERO,
        ) {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                return Err(failure(entry_error(
                    ExistingParentLockError::NamespaceChanged { path: ENTRY.into() },
                )));
            }
            Err(refused) => {
                let lease = refused.guard.map(|guard| {
                    Arc::new(Self {
                        inner: Admission {
                            root,
                            guard: Mutex::new(guard),
                        },
                    })
                });
                return Err(ExistingActivityFailure {
                    lease,
                    error: entry_error(refused.error),
                });
            }
        };
        let lease = Arc::new(Self {
            inner: Admission {
                root,
                guard: Mutex::new(guard),
            },
        });
        after_acquisition(&lease);
        if let Err(error) = lease.revalidate() {
            return Err(ExistingActivityFailure {
                lease: Some(lease),
                error,
            });
        }
        Ok(lease)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn recovery_lock_observation(
        &self,
    ) -> Result<crate::archive_recovery::RecoveryLockObservation, ActivityAdmissionError> {
        self.revalidate()?;
        let observation = self
            .inner
            .guard
            .lock()
            .map_err(|_| ActivityAdmissionError::Unavailable(ActivityUnavailable::Poisoned))?
            .recovery_observation(self.inner.root.as_ref(), OsStr::new(ENTRY))
            .map_err(entry_error)?;
        self.revalidate()?;
        Ok(observation)
    }

    pub fn try_acquire(root: Arc<JournalRoot>) -> Result<Arc<Self>, ActivityAdmissionError> {
        Ok(Arc::new(Self {
            inner: Admission::acquire(root)?,
        }))
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.inner.root
    }

    pub fn revalidate(&self) -> Result<(), ActivityAdmissionError> {
        self.inner.revalidate()
    }

    /// Share the held lock with a joined child; this does not grant settlement.
    /// Retain this lease until the child stops: its drop explicitly unlocks the shared lock.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn duplicate_descriptor_for_child(
        &self,
    ) -> Result<std::os::fd::OwnedFd, ActivityAdmissionError> {
        let root_error =
            |error| ActivityAdmissionError::Unavailable(ActivityUnavailable::Root(error));
        self.inner
            .root
            .revalidate_canonical_binding()
            .map_err(root_error)?;
        let guard = self
            .inner
            .guard
            .lock()
            .map_err(|_| ActivityAdmissionError::Unavailable(ActivityUnavailable::Poisoned))?;
        guard
            .revalidate_bound(self.inner.root.as_ref(), OsStr::new(ENTRY))
            .map_err(entry_error)?;
        let descriptor = guard
            .duplicate_descriptor(OsStr::new(ENTRY))
            .map_err(entry_error)?;
        guard
            .revalidate_bound(self.inner.root.as_ref(), OsStr::new(ENTRY))
            .map_err(entry_error)?;
        self.inner
            .root
            .revalidate_canonical_binding()
            .map_err(root_error)?;
        Ok(descriptor)
    }
}

/// Exclusive admission for fresh checks and cooperating destructive boundaries.
/// Only its opaque root carrier may reserve an empty stage; it grants no
/// writable media stage and does not certify an empty census.
pub struct TeardownLease {
    inner: Admission,
}

impl TeardownLease {
    pub fn try_acquire(root: Arc<JournalRoot>) -> Result<Arc<Self>, ActivityAdmissionError> {
        Ok(Arc::new(Self {
            inner: Admission::acquire(root)?,
        }))
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.inner.root
    }

    pub fn revalidate(&self) -> Result<(), ActivityAdmissionError> {
        self.inner.revalidate()
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;
