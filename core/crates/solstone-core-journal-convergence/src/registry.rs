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

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::access::RegistrySection;
use crate::error::{ConvergenceError, DurableRole};
use crate::layout::OWNERS;
use crate::walk::open_dir;

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
