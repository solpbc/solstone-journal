// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reentrant, journal-wide entity trust-operation locking.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::{self, ThreadId};

use solstone_core_journal_io::{FileLock, LockError, LockOptions, contained_path, hold_lock};

use solstone_core_journal_io::PathError;

const TRUST_LOCK_RELATIVE_PATH: &str = "health/locks/entity-trust";

/// Failure while acquiring the entity trust-operation lock.
#[derive(Debug)]
pub enum EntityTrustLockError {
    /// The journal-relative trust-lock path could not be contained safely.
    Path(PathError),
    /// The sidecar lock could not be acquired.
    Lock(LockError),
}

impl fmt::Display for EntityTrustLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
        }
    }
}

impl Error for EntityTrustLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Lock(error) => Some(error),
        }
    }
}

impl From<PathError> for EntityTrustLockError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<LockError> for EntityTrustLockError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

/// A held entity trust-operation lock.
///
/// Dropping the guard releases one nesting level. The guard is deliberately not
/// sendable because the coordinator tracks ownership by thread.
#[derive(Debug)]
pub struct EntityTrustLock {
    lock_path: PathBuf,
    _not_send: PhantomData<*const ()>,
}

struct TrustLockCoordinator {
    state: Mutex<HashMap<PathBuf, TrustLockState>>,
    available: Condvar,
}

enum TrustLockState {
    Acquiring {
        owner: ThreadId,
    },
    Held {
        owner: ThreadId,
        depth: usize,
        _lock: FileLock,
    },
}

static COORDINATOR: OnceLock<TrustLockCoordinator> = OnceLock::new();

/// Hold the journal-wide entity trust-operation lock.
///
/// This lock is reentrant for the owning thread, not for the process. Nested
/// calls reuse the same [`FileLock`] and increment a depth counter without
/// touching the sidecar again. A different thread, including one in this
/// process, waits until the outermost guard drops.
pub fn hold_entity_trust_lock(
    journal_root: &Path,
) -> Result<EntityTrustLock, EntityTrustLockError> {
    hold_entity_trust_lock_with_options(journal_root, LockOptions::default())
}

pub(crate) fn hold_entity_trust_lock_with_options(
    journal_root: &Path,
    options: LockOptions,
) -> Result<EntityTrustLock, EntityTrustLockError> {
    let lock_path = contained_path(journal_root, TRUST_LOCK_RELATIVE_PATH)?;
    let owner = thread::current().id();
    let coordinator = coordinator();

    let mut state = lock_state(coordinator);
    loop {
        match state.get_mut(&lock_path) {
            Some(TrustLockState::Held {
                owner: held_owner,
                depth,
                ..
            }) if *held_owner == owner => {
                *depth += 1;
                return Ok(EntityTrustLock::new(lock_path));
            }
            Some(TrustLockState::Acquiring {
                owner: acquiring_owner,
            }) if *acquiring_owner == owner => {
                unreachable!("an entity trust lock cannot reenter while acquiring")
            }
            Some(_) => {
                state = wait_for_state(coordinator, state);
            }
            None => {
                state.insert(lock_path.clone(), TrustLockState::Acquiring { owner });
                break;
            }
        }
    }
    drop(state);

    match hold_lock(&lock_path, options) {
        Ok(lock) => {
            let mut state = lock_state(coordinator);
            let previous = state.insert(
                lock_path.clone(),
                TrustLockState::Held {
                    owner,
                    depth: 1,
                    _lock: lock,
                },
            );
            debug_assert!(matches!(
                previous,
                Some(TrustLockState::Acquiring {
                    owner: acquiring_owner
                }) if acquiring_owner == owner
            ));
            coordinator.available.notify_all();
            Ok(EntityTrustLock::new(lock_path))
        }
        Err(error) => {
            let mut state = lock_state(coordinator);
            let previous = state.remove(&lock_path);
            debug_assert!(matches!(
                previous,
                Some(TrustLockState::Acquiring {
                    owner: acquiring_owner
                }) if acquiring_owner == owner
            ));
            coordinator.available.notify_all();
            Err(error.into())
        }
    }
}

impl EntityTrustLock {
    fn new(lock_path: PathBuf) -> Self {
        Self {
            lock_path,
            _not_send: PhantomData,
        }
    }
}

impl Drop for EntityTrustLock {
    fn drop(&mut self) {
        let coordinator = coordinator();
        let owner = thread::current().id();
        let mut state = lock_state(coordinator);
        let release = match state.get_mut(&self.lock_path) {
            Some(TrustLockState::Held {
                owner: held_owner,
                depth,
                ..
            }) if *held_owner == owner => {
                debug_assert!(*depth > 0);
                *depth -= 1;
                *depth == 0
            }
            _ => return,
        };
        if release {
            state.remove(&self.lock_path);
            coordinator.available.notify_all();
        }
    }
}

fn coordinator() -> &'static TrustLockCoordinator {
    COORDINATOR.get_or_init(|| TrustLockCoordinator {
        state: Mutex::new(HashMap::new()),
        available: Condvar::new(),
    })
}

fn lock_state(
    coordinator: &TrustLockCoordinator,
) -> MutexGuard<'_, HashMap<PathBuf, TrustLockState>> {
    coordinator
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait_for_state<'a>(
    coordinator: &'a TrustLockCoordinator,
    state: MutexGuard<'a, HashMap<PathBuf, TrustLockState>>,
) -> MutexGuard<'a, HashMap<PathBuf, TrustLockState>> {
    coordinator
        .available
        .wait(state)
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
