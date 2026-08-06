// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Telling the search index which paths were removed — **after** removing them.
//!
//! # The ordering is a safety property, not a preference
//!
//! Index discovery is a filesystem glob with no database input, and a scan deletes
//! any row the glob no longer produces. So the index re-converges on the chronicle
//! every run, and the two orderings fail differently:
//!
//! - **Remove, then tell.** A crash between them leaves rows for a file that is
//!   gone. A query returns the hit, something opens it, and the failure is loud,
//!   local, and on a code path that actually runs. The next scan clears it.
//! - **Tell, then remove.** A crash leaves files on disk the index does not list.
//!   Nothing surfaces that: a missing search result is indistinguishable from
//!   misremembering. It survives until someone runs a full rebuild, which nobody
//!   does on a system that looks healthy.
//!
//! For a journal whose promise is that the owner's recordings are theirs and
//! findable, the second is a silent loss of access to their own data. That is the
//! reason — stronger than any rule — and it is why this takes a
//! [`RemovedPath`](crate::receipt::RemovedPath), a value only the removal door can
//! mint and only after confirming a path is gone. Telling the index about a removal
//! that has not happened is not forbidden here; it is unrepresentable.
//!
//! ⚠ **This is the inverse of what a content-addressed store does**, and a reader
//! who knows that will want to "correct" it. The difference is which side is
//! authoritative: there the index is authoritative and the blobs are derived, so
//! the index is updated first. Here the chronicle is authoritative and the index is
//! a rebuildable cache. Both obey the same rule — update the authority first.
//!
//! ⛔ **And the index is never an input to a removal decision.** The executor
//! decides from the filesystem; the index only ever learns afterwards. That is what
//! makes "derived cache" true rather than aspirational.

use crate::receipt::RemovedPath;

/// How many index rows a notification cleared.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PruneCounts {
    pub chunks: u64,
    pub files: u64,
}

/// What a notification could not do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotifyError {
    pub reason: String,
}

/// The search index, as the removal executor needs to address it.
///
/// Implemented outside this crate, by whoever owns the index — so retention
/// depends on an interface rather than on a database, and the contract can move
/// without touching the executor.
pub trait IndexNotify {
    /// Drop rows for paths that are **already gone** from the chronicle.
    ///
    /// ⛔ Must tolerate a path the index never held: the caller's authority is the
    /// filesystem, and being told about an unindexed path is ordinary. ⛔ And must
    /// not create an index — a prune is not a reason to bring one into existence.
    fn paths_removed(&self, removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError>;
}

/// A notification target that does nothing, for a journal with no index.
///
/// ⚠ Not a stub for tests to lean on: a journal genuinely need not have an index,
/// and the executor must not treat its absence as a failure.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoIndex;

impl IndexNotify for NoIndex {
    fn paths_removed(&self, _removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError> {
        Ok(PruneCounts::default())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use super::*;

    /// A notification cannot name a path that was not removed.
    ///
    /// There is no way to build the argument except through the removal door, so
    /// this is a compile-time property. The test documents it: the only public
    /// route to a `RemovedPath` is a verb's outcome.
    #[test]
    fn a_notification_can_only_name_paths_a_removal_produced() {
        let no_index = NoIndex;
        // An empty notification is legal and does nothing.
        assert_eq!(no_index.paths_removed(&[]).unwrap(), PruneCounts::default());
        // Anything non-empty must come from an Outcome's `removed` list; there is
        // no constructor reachable from here.
    }
}
