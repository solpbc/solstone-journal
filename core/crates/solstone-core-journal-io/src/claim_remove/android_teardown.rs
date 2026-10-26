// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Android no-replace removal admitted only by the original teardown lease.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::activity::TeardownLease;

pub(super) struct Authority {
    lease: Arc<TeardownLease>,
    relative: PathBuf,
}

impl Authority {
    pub(super) fn revalidate(&self, directory: &FlatDirectory) -> io::Result<()> {
        self.lease.revalidate().map_err(io::Error::other)?;
        let current = FlatDirectory::open(self.lease.journal_root(), &self.relative)
            .map_err(io::Error::other)?;
        directory.revalidate().map_err(io::Error::other)?;
        if current.identity() != directory.identity() {
            return Err(io::Error::other("teardown claim parent changed"));
        }
        self.lease.revalidate().map_err(io::Error::other)
    }
}

/// Remove an exact observed member under the retained Android teardown lease.
///
/// `relative` is a portable parent path beneath that lease's retained root.
/// The already-held parent must still be that exact descendant at each native
/// mutation. This does not admit generic Android rename or claim/removal.
pub fn claim_and_remove_android_teardown_observed_bounded(
    lease: &Arc<TeardownLease>,
    directory: &FlatDirectory,
    relative: &Path,
    original: &OsStr,
    prior: &FileObservation,
    claim: &ClaimName,
    maximum: usize,
) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
    let authority = Authority {
        lease: Arc::clone(lease),
        relative: relative.to_path_buf(),
    };
    authority
        .revalidate(directory)
        .map_err(|source| ClaimRemovalError::Io {
            operation: "revalidate Android teardown claim parent",
            path: directory.diagnostic_entry(original),
            source,
        })?;
    claim_and_remove_with_policy(
        directory,
        original,
        prior,
        claim,
        maximum,
        &RenamePolicy::AndroidTeardown(authority),
    )
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;
