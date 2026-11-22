// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Social memory: an owner's journal shares an activity with the people in
//! their life as a record in an atproto space on the owner's own host, keeps
//! the shared copy in step with the activity, and removes it on unshare. A
//! receiving journal keeps what others shared with it apart from its own
//! memories, marked shared and attributed to whoever shared it.
//!
//! Every network hop goes between the journal and atproto hosts: the owner's
//! own PDS, the sharer's PDS and the identity directory. The OAuth client is
//! the loopback form, so signing in fetches no client document from anywhere.
//!
//! The record and space-type names are provisional until the lexicon is
//! published.

#[cfg(feature = "dev-command")]
pub mod cli;
pub mod credential;
pub mod dns;
pub mod record;
#[cfg(feature = "dev-command")]
pub mod rig;
pub mod space;
pub mod store;

/// The activity record's collection.
pub const ACTIVITY_COLLECTION: &str = "app.solstone.memory.activity";

/// The space type the journal creates for a share.
pub const SPACE_TYPE: &str = "app.solstone.memory.space";

/// The OAuth scope the journal requests: the atproto base scope; managing
/// spaces it is the authority for and writing activity records into them;
/// and reading spaces others have added this account to.
///
/// Both space permissions name the space type as `*`: a concrete type is
/// resolved against its published declaration when the owner signs in, and
/// the type is not published yet.
pub fn sign_in_scope() -> String {
    format!(
        "atproto space:*?collection={ACTIVITY_COLLECTION}&manage=create&manage=update&manage=delete space:*?authority=*&action=read"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_names_no_concrete_space_type() {
        let scope = sign_in_scope();
        assert!(scope.split_whitespace().any(|s| s == "atproto"));
        for space in scope.split_whitespace().filter(|s| s.starts_with("space:")) {
            assert!(space.starts_with("space:*?"), "{space}");
        }
    }

    #[test]
    fn received_memories_are_offered_to_no_reader() {
        use solstone_core_format::content::{IndexDisposition, OfferedTo, file_spec};
        for path in [
            "social/received/0123abcd/3lb2abcdefg22.json",
            "social/received/0123abcd/space.json",
            "social/sent/shares.json",
        ] {
            let spec = file_spec(path).expect("social paths are registered");
            assert_eq!(spec.pattern, "social/**", "{path}");
            assert_eq!(spec.offered_to, OfferedTo::NONE, "{path}");
            assert_eq!(spec.disposition, IndexDisposition::Excluded, "{path}");
        }
    }
}
