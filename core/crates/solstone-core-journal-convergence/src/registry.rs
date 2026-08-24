// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Brief registry sections.
//!
//! Lock order: days → brief global (`topology.lock`) → release global →
//! registry (`registry.lock`) → release registry. Registry never overlaps
//! `topology.lock`. Registry is never held while acquiring or waiting on a
//! day lock. A section exposes only the registry directory: day-artifact
//! scans and other lock namespaces are not reachable through it.

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::time::Duration;

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::error::{ConvergenceError, DurableRole};
use crate::init::StoreDirs;
use crate::layout::OWNERS;
use crate::lock::{LOCK_TIMEOUT, RegistryGuard, hold_registry_with_timeout};
use crate::walk::open_dir;

/// Live registry section. Dropping it releases `registry.lock`.
pub(crate) struct RegistrySection<'a> {
    _guard: RegistryGuard,
    dirs: &'a StoreDirs,
}

pub(crate) fn enter_registry(dirs: &StoreDirs) -> Result<RegistrySection<'_>, ConvergenceError> {
    enter_registry_with_timeout(dirs, LOCK_TIMEOUT)
}

pub(crate) fn enter_registry_with_timeout(
    dirs: &StoreDirs,
    timeout: Duration,
) -> Result<RegistrySection<'_>, ConvergenceError> {
    let guard = hold_registry_with_timeout(dirs, timeout)?;
    Ok(RegistrySection {
        _guard: guard,
        dirs,
    })
}

impl<'a> RegistrySection<'a> {
    pub(crate) fn registry(&self) -> &'a OwnedFd {
        &self.dirs.registry
    }
}

pub(crate) fn ensure_owners_dir(
    section: &RegistrySection<'_>,
) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(section.registry(), OsStr::new(OWNERS), 0o700).map_err(|error| {
        ConvergenceError::Io {
            operation: "create owners directory",
            role: DurableRole::Directory,
            source: std::io::Error::other(error.to_string()),
        }
    })?;
    open_dir(section.registry(), OWNERS)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

pub(crate) fn sync_owners(owners: &OwnedFd) -> Result<(), ConvergenceError> {
    sync_dir_bound(owners).map_err(|source| ConvergenceError::Io {
        operation: "sync owners directory",
        role: DurableRole::PreparedOwner,
        source,
    })
}
