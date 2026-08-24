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

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::access::ResolverAccess;
    use crate::test_support::admit_days;

    #[test]
    fn ensure_and_sync_owners_is_idempotent_under_the_registry_section() {
        let (_temporary, admitted) = admit_days("registry-owners", &["20260823"]);
        let access = ResolverAccess::acquire(&admitted).unwrap();
        access
            .with_registry(|section| {
                let owners = ensure_owners_dir(section)?;
                sync_owners(&owners)?;
                let again = ensure_owners_dir(section)?;
                sync_owners(&again)
            })
            .unwrap();
    }

    #[test]
    fn owners_path_that_is_not_a_directory_is_an_io_error() {
        let (temporary, admitted) = admit_days("registry-owners-file", &["20260823"]);
        let owners = temporary
            .journal_path()
            .join("health/convergence/registry/owners");
        std::fs::remove_dir(&owners).unwrap();
        std::fs::write(&owners, b"not-a-directory").unwrap();
        let access = ResolverAccess::acquire(&admitted).unwrap();
        assert!(matches!(
            access.with_registry(ensure_owners_dir),
            Err(ConvergenceError::Io {
                role: DurableRole::Directory,
                ..
            })
        ));
    }
}
