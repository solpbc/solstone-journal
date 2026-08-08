// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::digest::EMPTY_CONTENT_SHA256;
use crate::{BodyDigest, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField};

const LEDGER_PATH: &str = "body-ledger.jsonl";

/// Checked native body-envelope values for its ledger sidecar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeLedger {
    path: &'static str,
    bytes: u64,
    events: u64,
    sha256: BodyDigest,
}

impl EnvelopeLedger {
    /// Binds checked native body-envelope values for its ledger sidecar.
    pub fn new(
        bundle: &BundleId,
        bytes: u64,
        events: u64,
        sha256: BodyDigest,
    ) -> Result<Self, EnvelopeError> {
        if (events == 0) != (bytes == 0) {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::LedgerBytes,
                None,
            ));
        }
        if events > bytes {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::LedgerEvents,
                None,
            ));
        }
        if (sha256.as_str() == EMPTY_CONTENT_SHA256) != (bytes == 0) {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::LedgerSha256,
                None,
            ));
        }

        Ok(Self {
            path: LEDGER_PATH,
            bytes,
            events,
            sha256,
        })
    }

    /// Returns this ledger's fixed relative path.
    pub fn path(&self) -> &str {
        self.path
    }

    /// Returns this ledger's declared byte count.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Returns this ledger's declared event count.
    pub fn events(&self) -> u64 {
        self.events
    }

    /// Returns this ledger's checked SHA-256 digest.
    pub fn sha256(&self) -> &BodyDigest {
        &self.sha256
    }
}
