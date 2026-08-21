// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native paired-peer journal transfer.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod manifest;
mod peer;
mod peer_export;
mod rescan;
mod send;

use thiserror::Error;

pub use peer::{ResolvedPeer, UnpairOutcome, resolve_peer, unpair_peer};
pub use peer_export::{PeerExportAreaResult, PeerExportReport, PeerExportRequest, peer_export};
pub use rescan::{RescanOutcome, send_indexer_rescan};

/// Transfer validation, source, or publication error.
#[derive(Debug, Error)]
pub enum TransferError {
    /// The supplied day is invalid.
    #[error("day must be YYYYMMDD")]
    InvalidDay,
    /// `journal export --only` named no valid export area.
    #[error("invalid export area selection")]
    InvalidExportAreas,
    /// No paired peers are available in this journal.
    #[error("no peers paired (run \"solstone link join --as peer\" first)")]
    NoPeersPaired,
    /// No peer has the requested label.
    #[error("no peer with label \"{label}\"; available: {available}")]
    PeerNotFound { label: String, available: String },
    /// More than one peer has the requested label.
    #[error(
        "multiple peers with label \"{label}\": {instance_ids}; use <journal_root>/peers/<instance_id> directly"
    )]
    AmbiguousPeer { label: String, instance_ids: String },
    /// Paired-link identity files or configuration could not be loaded.
    #[error("paired-link credential load failed: {0}")]
    CredentialLoad(String),
    /// Carrier or loopback HTTP transport failed.
    #[error("paired-link transport failed: {0}")]
    Transport(String),
    /// The local paired-link bridge could not start or drain.
    #[error("paired-link bridge failed: {0}")]
    Bridge(String),
    /// Archive input or filesystem operation failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Journal path validation failed.
    #[error("{0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    /// Manifest JSON or shape is invalid.
    #[error("invalid transfer manifest: {0}")]
    Manifest(String),
    /// A peer export manifest query was rejected by the remote journal.
    #[error("{0}")]
    ManifestQuery(String),
}
