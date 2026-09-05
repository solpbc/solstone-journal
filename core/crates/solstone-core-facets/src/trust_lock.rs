// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reentrant, journal-wide facet mutation locking.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::{self, ThreadId};

use solstone_core_journal_io::{
    FileLock, LockError, LockOptions, PathError, contained_path, hold_lock,
};

const TRUST_LOCK_RELATIVE_PATH: &str = "health/locks/facet-trust";

/// Failure while acquiring the facet trust-operation lock.
#[derive(Debug)]
pub enum FacetTrustLockError {
    Path(PathError),
    Lock(LockError),
}

impl fmt::Display for FacetTrustLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
        }
    }
}

impl Error for FacetTrustLockError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Lock(error) => Some(error),
        }
    }
}

impl From<PathError> for FacetTrustLockError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<LockError> for FacetTrustLockError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

/// A held facet trust-operation lock.
#[derive(Debug)]
pub struct FacetTrustLock {
    lock_path: PathBuf,
    _entity: solstone_core_entity::EntityTrustLock,
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

/// Hold the journal-wide facet trust-operation lock.
pub fn hold_facet_trust_lock(journal_root: &Path) -> Result<FacetTrustLock, FacetTrustLockError> {
    hold_facet_trust_lock_with_options(journal_root, LockOptions::default())
}

pub(crate) fn hold_facet_trust_lock_with_options(
    journal_root: &Path,
    options: LockOptions,
) -> Result<FacetTrustLock, FacetTrustLockError> {
    // Entity merges also rewrite facet memory. Every nested acquisition uses
    // entity -> facet -> individual artifact, including calls from entity code.
    let entity =
        solstone_core_entity::hold_entity_trust_lock(journal_root).map_err(
            |error| match error {
                solstone_core_entity::EntityTrustLockError::Path(error) => {
                    FacetTrustLockError::Path(error)
                }
                solstone_core_entity::EntityTrustLockError::Lock(error) => {
                    FacetTrustLockError::Lock(error)
                }
            },
        )?;
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
                return Ok(FacetTrustLock::new(lock_path, entity));
            }
            Some(TrustLockState::Acquiring {
                owner: acquiring_owner,
            }) if *acquiring_owner == owner => {
                unreachable!("a facet trust lock cannot reenter while acquiring")
            }
            Some(_) => state = wait_for_state(coordinator, state),
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
            Ok(FacetTrustLock::new(lock_path, entity))
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

/// Acquire the raw facet trust-lock file directly, bypassing the in-process
/// reentrant coordinator `hold_facet_trust_lock` uses. Exists so integration
/// tests in other crates can simulate the trust lock already held by another
/// process; production code must go through `hold_facet_trust_lock`.
pub fn hold_facet_trust_lock_raw_for_test(
    journal_root: &Path,
) -> Result<FileLock, FacetTrustLockError> {
    let lock_path = contained_path(journal_root, TRUST_LOCK_RELATIVE_PATH)?;
    Ok(hold_lock(&lock_path, LockOptions::default())?)
}

impl FacetTrustLock {
    fn new(lock_path: PathBuf, entity: solstone_core_entity::EntityTrustLock) -> Self {
        Self {
            lock_path,
            _entity: entity,
            _not_send: PhantomData,
        }
    }
}

impl Drop for FacetTrustLock {
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
