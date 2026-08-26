// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The only artifact that survives a removal, and what it may honestly say.
//!
//! A removed segment is emptied and left holding one file. That file is the whole
//! of what an owner, or an auditor, or a future maintainer can ever learn about
//! what happened — so it is the most consequential thing this crate writes, and
//! the temptation it must resist is claiming more than it did.
//!
//! # Why this lives here rather than at the segment boundary
//!
//! This crate is the only tombstone writer. The segment crate's superseded one —
//! which hard-coded `reason: "owner_location_data_delete"` from a time when
//! deleting a *source* was an operation — was removed. An owner deletes segments,
//! so the reason vocabulary is this crate's closed set ([`RemovalReason`]), never
//! a free string and never a source-level claim.
//!
//! ⛔ The contents live here with the executor that owns every removal: a staged
//! directory cannot be named by a resolved segment handle, because that boundary
//! rejects a leading-dot component.
//!
//! # What it may claim
//!
//! Nothing about recoverability. An unlink does not reach the storage medium, and
//! the recognised sanitization scale starts *above* what this does — so the level
//! is stated as unachieved rather than left out, because a missing field reads as
//! an unasked question and a null one reads as a declined claim.
//!
//! ⛔ Never the words *proof*, *verified*, *permanently* or *unrecoverable*. No
//! classical protocol can establish that a copy was not made, so this is a
//! **receipt** — a record of an action taken on a named scope — and calling it
//! anything stronger is the gap between a promise and a mechanism.

use serde::Serialize;

/// The standard whose vocabulary the level below is quoted from.
const SANITIZATION_STANDARD: &str = "NIST SP 800-88r1";

/// What this removal achieved on that standard's scale.
///
/// 🔴 `None`, and deliberately serialized as `null` rather than omitted. Removing
/// a directory entry does not reach the storage medium: the weakest recognised
/// tier still assumes the data was overwritten, and on flash media even that is
/// documented as not reaching every cell. **The claim declined is what makes the
/// rest of the file credible.**
const SANITIZATION_LEVEL: Option<&str> = None;

/// The operation performed, in the vocabulary that already distinguishes it.
///
/// The published data-privacy vocabulary separates *delete* — removal with the
/// possibility of retrieval — from *erase*, removal from existence. This is the
/// former, and saying so in a term someone else defined is more honest than prose.
const OPERATION: &str = "dpv:Delete";

/// Why a segment was removed.
///
/// ⛔ A closed set, never a free string. The superseded writer's single hard-coded
/// value is exactly the failure a free string invites: a durable owner-facing
/// claim that outlived the feature it described.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalReason {
    /// The owner asked for these segments to be deleted.
    ///
    /// ⚠ Segments. Source-delete resolves a source name to a set of whole
    /// segments and still uses this reason. There is no partial owner-directed
    /// delete, so there is no reason variant naming a file-level erase.
    OwnerSegmentDelete,
    /// The configured retention policy reached them.
    RetentionPolicy,
}

/// Who performed the removal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutorStamp {
    pub name: &'static str,
    pub version: &'static str,
}

impl Default for ExecutorStamp {
    fn default() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// The tombstone's contents.
///
/// ⚠ The three honesty fields are **not** in here on purpose. They are invariants
/// of what this crate does, not choices a caller makes, and a caller able to pass
/// `sanitization_level: Some("Clear")` could make the one claim this file exists to
/// decline — while any test asserting the field's presence stayed green.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TombstoneBody {
    pub deleted_at: String,
    pub cid: String,
    pub reason: RemovalReason,
    /// Every journal-relative path removed from this segment.
    ///
    /// This is what lets a later pass recognise a segment restored from a backup
    /// and remove it again — the one genuinely useful thing a receipt can promise
    /// an owner about copies it does not control.
    pub manifest: Vec<String>,
}

/// What is actually written, honesty fields included.
#[derive(Serialize)]
struct Written<'a> {
    deleted_at: &'a str,
    #[serde(rename = "cid")]
    cid: &'a str,
    reason: RemovalReason,
    manifest: &'a [String],
    manifest_count: usize,
    sanitization_standard: &'static str,
    sanitization_level: Option<&'static str>,
    operation: &'static str,
    executor: ExecutorStamp,
}

/// Serialize a tombstone.
///
/// 🔴 Takes no segment handle, which is what lets the removal executor write into
/// a staged directory the segment boundary cannot name. One serializer, so there
/// is no second place for the shape to drift to.
pub fn tombstone_bytes(body: &TombstoneBody) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(&Written {
        deleted_at: &body.deleted_at,
        cid: &body.cid,
        reason: body.reason,
        manifest: &body.manifest,
        manifest_count: body.manifest.len(),
        sanitization_standard: SANITIZATION_STANDARD,
        sanitization_level: SANITIZATION_LEVEL,
        operation: OPERATION,
        executor: ExecutorStamp::default(),
    })
}

/// The filename a tombstone is written as.
pub const TOMBSTONE_NAME: &str = "tombstone.json";

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

    fn body() -> TombstoneBody {
        TombstoneBody {
            deleted_at: "2026-08-05T21:00:00Z".to_owned(),
            cid: "sha256:abc".to_owned(),
            reason: RemovalReason::OwnerSegmentDelete,
            manifest: vec![
                "chronicle/20260805/field.audio/070000_17/audio.flac".to_owned(),
                "chronicle/20260805/field.audio/070000_17/audio.jsonl".to_owned(),
            ],
        }
    }

    #[test]
    fn carries_the_reason_it_was_given_for_both_members_of_the_closed_set() {
        for (reason, rendered) in [
            (RemovalReason::OwnerSegmentDelete, "owner_segment_delete"),
            (RemovalReason::RetentionPolicy, "retention_policy"),
        ] {
            let mut body = body();
            body.reason = reason;
            let json = String::from_utf8(tombstone_bytes(&body).unwrap()).unwrap();
            assert!(json.contains(rendered), "expected {rendered} in {json}");
        }
    }

    #[test]
    fn names_every_removed_path_and_counts_them() {
        let json: serde_json::Value =
            serde_json::from_slice(&tombstone_bytes(&body()).unwrap()).unwrap();
        let manifest = json["manifest"].as_array().unwrap();
        assert_eq!(manifest.len(), 2);
        assert_eq!(json["manifest_count"], 2);
    }

    /// The declined claim is present and null, not absent.
    ///
    /// A missing field reads as a question nobody asked; a null one reads as a
    /// claim deliberately not made. That difference is the whole point.
    #[test]
    fn the_sanitization_level_is_present_and_declined() {
        let json: serde_json::Value =
            serde_json::from_slice(&tombstone_bytes(&body()).unwrap()).unwrap();
        assert!(
            json.as_object().unwrap().contains_key("sanitization_level"),
            "the declined claim must be present, not omitted"
        );
        assert!(json["sanitization_level"].is_null());
        assert_eq!(json["sanitization_standard"], "NIST SP 800-88r1");
        assert_eq!(json["operation"], "dpv:Delete");
    }

    #[test]
    fn stamps_the_executor_and_its_version() {
        let json: serde_json::Value =
            serde_json::from_slice(&tombstone_bytes(&body()).unwrap()).unwrap();
        assert_eq!(json["executor"]["name"], "solstone-core-retention");
        assert!(
            !json["executor"]["version"].as_str().unwrap().is_empty(),
            "a tool stamp without a version cannot identify what ran"
        );
    }

    /// ⛔ The never-list, over the bytes an owner could read.
    ///
    /// A receipt is a record of an action on a named scope. No classical protocol
    /// can establish that a copy was not made, so any of these words would be the
    /// gap between a promise and a mechanism, written durably into the owner's own
    /// journal.
    #[test]
    fn never_claims_more_than_it_did() {
        let json = String::from_utf8(tombstone_bytes(&body()).unwrap()).unwrap();
        let lowered = json.to_lowercase();
        for forbidden in ["proof", "verified", "permanently", "unrecoverable"] {
            assert!(
                !lowered.contains(forbidden),
                "the tombstone must never say `{forbidden}`: {json}"
            );
        }
    }

    /// A caller cannot make the claim this file exists to decline.
    #[test]
    fn a_caller_cannot_supply_the_honesty_fields() {
        // TombstoneBody has exactly four fields, and none of them is a claim
        // about sanitization. This is a compile-time property; the assertion
        // documents it so a future field addition has to argue with it.
        let TombstoneBody {
            deleted_at: _,
            cid: _,
            reason: _,
            manifest: _,
        } = body();
    }
}
