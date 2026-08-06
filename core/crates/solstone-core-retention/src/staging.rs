// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The staged name a segment wears while it is being removed.
//!
//! Emptying a directory is not atomic, so a crash part way through would leave a
//! half-removed segment sitting in the chronicle under its real name — a shape
//! every reader would treat as ordinary. The removal therefore moves the segment
//! aside in **one rename**, empties it there, and moves it back holding only its
//! tombstone. The chronicle only ever shows the segment whole or gone.
//!
//! # Why a prefix, and why in the same directory
//!
//! The journal recognises a segment by **scanning** its directory name for the key
//! pattern at a word boundary, not by matching the whole name. So a *trailing*
//! decoration — `<dir>.removing`, which is the obvious choice and what comparable
//! systems use — leaves the directory still recognised as a segment, **under its
//! undecorated key**, and a segment iterator returns two entries with one key and
//! two paths. A leading `.removing_` is not recognised, because the character
//! before the digits is a word character and the boundary test fails.
//!
//! ⛔ Both facts are pinned in a committed cross-language fixture. Do not
//! "simplify" this to a suffix.
//!
//! Staging in the segment's **own parent** matters too: a separate tree elsewhere
//! in the journal would make this a cross-directory rename, and some filesystems
//! reorder directory operations across different directories. One parent also means
//! one directory to flush.

/// The prefix a segment wears while its contents are being removed.
pub const STAGED_PREFIX: &str = ".removing_";

/// The staged name for a segment directory.
///
/// ⛔ Derived from the directory **name**, never from a key parsed out of it: the
/// two differ, and a staged name built from a key restores under the wrong name.
pub fn staged_name(dir: &str) -> String {
    format!("{STAGED_PREFIX}{dir}")
}

/// The original directory name a staged name was made from.
pub fn original_name(staged: &str) -> Option<&str> {
    staged.strip_prefix(STAGED_PREFIX).filter(|rest| {
        // ⛔ A staged name is provenance, and a name alone is weak provenance, so
        // the recovered original must at least look like a segment this crate
        // could have staged. Without this, recovery would empty any directory
        // someone happened to name `.removing_something`.
        !rest.is_empty() && !rest.contains('/') && *rest != "." && *rest != ".."
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_name_is_a_prefix_and_round_trips() {
        assert_eq!(staged_name("070000_17"), ".removing_070000_17");
        assert_eq!(original_name(".removing_070000_17"), Some("070000_17"));
    }

    /// The key is not the directory name, and staging must preserve the name.
    #[test]
    fn a_suffixed_directory_name_round_trips_whole() {
        // The journal's scan would read `093000_300` out of this. Staging on the
        // key would restore the wrong directory.
        let dir = "093000_300_summary";
        assert_eq!(original_name(&staged_name(dir)), Some(dir));
    }

    #[test]
    fn a_name_that_could_not_have_been_staged_is_refused() {
        for name in [
            "070000_17",
            ".removing_",
            ".removing_.",
            ".removing_..",
            ".removing_a/b",
            "removing_070000_17",
        ] {
            assert!(original_name(name).is_none(), "{name:?}");
        }
    }
}
