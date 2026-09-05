// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Command identities shared by the distinct `sol` and `journal` executables.
//!
//! This crate contains names only. It grants neither API transport nor local
//! journal authority, so either executable may use it without crossing the
//! process boundary.

mod generated;
mod mover_tombstones;

pub use generated::{JOURNAL_HOST_COMMAND_COUNT, JOURNAL_HOST_COMMANDS};
pub use mover_tombstones::{
    JOURNAL_EXPORT_TOMBSTONE, TRANSFER_EXPORT_TOMBSTONE, TRANSFER_IMPORT_TOMBSTONE,
};

/// The usage line native `journal describe` prints for an argument error.
/// It belongs in the names-only boundary so the standalone media helper does
/// not depend on the full command router just to render its owner-facing verb.
pub const DESCRIBE_USAGE: &str = "usage: journal describe [-h] [--frames-only] [--redo] [-j N] [--journal PATH] [-v] [-d] FILE\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_commands_are_sorted_unique_and_counted() {
        assert_eq!(JOURNAL_HOST_COMMANDS.len(), JOURNAL_HOST_COMMAND_COUNT);
        assert!(
            JOURNAL_HOST_COMMANDS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
    }
}
