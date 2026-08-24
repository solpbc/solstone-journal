// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Brief registry sections.
//!
//! Lock order: days → brief global (`topology.lock`) → release global →
//! registry (`registry.lock`) → release registry. Registry never overlaps
//! `topology.lock`. Registry is never held while acquiring or waiting on a
//! day lock. A section exposes only the registry directory: day-artifact
//! scans and other lock namespaces are not reachable through it.

use std::os::fd::OwnedFd;
use std::time::Duration;

use crate::error::ConvergenceError;
use crate::init::StoreDirs;
use crate::lock::{LOCK_TIMEOUT, RegistryGuard, hold_registry_with_timeout};

/// Live registry section. Dropping it releases `registry.lock`.
// Wired by hook A in the next commit (prepared-owner issuance).
#[allow(dead_code)]
pub(crate) struct RegistrySection<'a> {
    _guard: RegistryGuard,
    dirs: &'a StoreDirs,
}

// Wired by hook A in the next commit (prepared-owner issuance).
#[allow(dead_code)]
pub(crate) fn enter_registry(dirs: &StoreDirs) -> Result<RegistrySection<'_>, ConvergenceError> {
    enter_registry_with_timeout(dirs, LOCK_TIMEOUT)
}

// Wired by hook A in the next commit (prepared-owner issuance).
#[allow(dead_code)]
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
    // Wired by hook A in the next commit (prepared-owner issuance).
    #[allow(dead_code)]
    pub(crate) fn registry(&self) -> &'a OwnedFd {
        &self.dirs.registry
    }
}
