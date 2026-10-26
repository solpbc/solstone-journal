// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Android obligation removal under the retained, drained restore target.

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::JournalRoot;
use crate::whole_journal_restore::{
    WholeJournalRestore, restore_target_quiescent, retirement_claim_name,
    whole_journal_restore_matches,
};

pub(super) struct Authority {
    root: Arc<JournalRoot>,
}

impl Authority {
    pub(super) fn revalidate(&self, directory: &FlatDirectory) -> io::Result<()> {
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        if !restore_target_quiescent(self.root.canonical_path()) {
            return Err(io::Error::other("restore target is not held and drained"));
        }
        let current =
            FlatDirectory::open(&self.root, Path::new("health")).map_err(io::Error::other)?;
        directory.revalidate().map_err(io::Error::other)?;
        if current.identity() != directory.identity() {
            return Err(io::Error::other("restore claim parent changed"));
        }
        let root_stat = nix::sys::stat::fstat(&*self.root)?;
        let health_stat = nix::sys::stat::fstat(directory)?;
        let magic = nix::sys::statfs::fstatfs(directory)?.filesystem_type().0 as u64;
        let process = crate::media::android_process_name()?;
        let path = self
            .root
            .canonical_path()
            .to_str()
            .ok_or_else(|| io::Error::other("restore root path is not unicode"))?;
        if root_stat.st_dev != health_stat.st_dev
            || !crate::media::qualify_android_container(magic, path, &process)
        {
            return Err(io::Error::other(
                "restore root is not a qualified private container",
            ));
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)
    }
}

/// The only Android restore caller may retire its exact local obligation.
/// This does not admit ordinary Android claim/removal or a teardown lease.
pub(crate) fn claim_and_remove_android_restore_observed_bounded(
    root: &Arc<JournalRoot>,
    directory: &FlatDirectory,
    prior: &FileObservation,
    claim: &ClaimName,
    maximum: usize,
) -> Result<ClaimRemovalOutcome, ClaimRemovalError> {
    let original = OsStr::new("whole-journal-restore.json");
    let record: WholeJournalRestore =
        serde_json::from_slice(&prior.bytes).map_err(|error| ClaimRemovalError::Io {
            operation: "read restore retirement attempt",
            path: directory.diagnostic_entry(original),
            source: io::Error::other(error),
        })?;
    if prior.entry.name.as_os_str() != original
        || claim.as_str() != retirement_claim_name(record.attempt)
        || !whole_journal_restore_matches(root.canonical_path(), &record)
    {
        return Err(ClaimRemovalError::Io {
            operation: "bind restore retirement attempt",
            path: directory.diagnostic_entry(original),
            source: io::Error::other("restore obligation changed"),
        });
    }
    let authority = Authority {
        root: Arc::clone(root),
    };
    authority
        .revalidate(directory)
        .map_err(|source| ClaimRemovalError::Io {
            operation: "admit Android restore retirement",
            path: directory.diagnostic_entry(original),
            source,
        })?;
    claim_and_remove_with_policy(
        directory,
        original,
        prior,
        claim,
        maximum,
        &RenamePolicy::AndroidRestore(authority),
    )
}

/// Restore one exact retained obligation claim before adopting the retry.
pub(crate) fn restore_android_restore_claim(
    root: &Arc<JournalRoot>,
    directory: &FlatDirectory,
    prior: &FileObservation,
    maximum: usize,
) -> io::Result<()> {
    let record: WholeJournalRestore =
        serde_json::from_slice(&prior.bytes).map_err(io::Error::other)?;
    if prior.entry.name != OsStr::new(&retirement_claim_name(record.attempt))
        || !whole_journal_restore_matches(root.canonical_path(), &record)
    {
        return Err(io::Error::other("retained restore claim changed"));
    }
    let authority = Authority {
        root: Arc::clone(root),
    };
    authority.revalidate(directory)?;
    match read_observed_file_unchecked_bounded(directory, &prior.entry.name, maximum)
        .map_err(io::Error::other)?
    {
        Some(current) if same_observation(&current, prior) => {}
        _ => {
            return Err(io::Error::other(
                "retained restore claim changed before retry",
            ));
        }
    }
    authority.revalidate(directory)?;
    android_native::rename_no_replace(
        directory,
        &prior.entry.name,
        OsStr::new("whole-journal-restore.json"),
    )
}
