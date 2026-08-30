// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Persistent, per-alias Windows locks for managed-log reference publication.

#![allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]

use std::ffi::{OsStr, OsString};

use crate::errors::ExistingParentLockError;
use crate::locking::{BoundParentLock, LockOptions, acquire_existing_parent_lock_bound};
use crate::managed_log_lock_boundary::{ManagedLogAliasLockBoundary, acquire_with_boundary};
use crate::managed_log_names::{ManagedLogAliasRole, alias_lock_name};
use crate::windows_sync_dir::WindowsFlatDirectory;

/// A held lock is tied to the retained alias parent that authorized it.
#[derive(Debug)]
pub(crate) struct ManagedLogAliasLock {
    lock: BoundParentLock,
    lock_name: OsString,
}

impl ManagedLogAliasLock {
    pub(crate) fn bound_parent_lock(&self) -> &BoundParentLock {
        &self.lock
    }

    pub(crate) fn lock_name(&self) -> &OsStr {
        &self.lock_name
    }
}

/// Acquire the stable root or day alias lock beneath an already-bound parent.
///
/// Root callers pass [`ManagedLogAliasRole::Root`], so a lock is retained across
/// day changes and never unlinked when this guard drops.
pub(crate) fn acquire_managed_log_alias_lock(
    directory: &WindowsFlatDirectory,
    role: ManagedLogAliasRole,
    logical_name: &str,
    options: LockOptions,
) -> Result<ManagedLogAliasLock, ExistingParentLockError> {
    let lock_name = alias_lock_name(role, logical_name);
    let boundary = WindowsManagedLogAliasLockBoundary { directory };
    let lock = acquire_with_boundary(&boundary, &lock_name, options)?;
    Ok(ManagedLogAliasLock { lock, lock_name })
}

struct WindowsManagedLogAliasLockBoundary<'a> {
    directory: &'a WindowsFlatDirectory,
}

impl ManagedLogAliasLockBoundary for WindowsManagedLogAliasLockBoundary<'_> {
    type Guard = BoundParentLock;

    fn acquire(
        &self,
        lock_name: &OsStr,
        options: LockOptions,
    ) -> Result<Self::Guard, ExistingParentLockError> {
        acquire_existing_parent_lock_bound(
            self.directory,
            lock_name,
            options.timeout,
            options.poll_interval,
        )
    }
}
