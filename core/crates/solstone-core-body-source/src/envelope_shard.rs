// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    BodyDigest, BodyMonth, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField,
};

const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Checked native body-envelope values for one normalized shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeShard {
    path: String,
    month: BodyMonth,
    bytes: u64,
    rows: u64,
    sha256: BodyDigest,
}

impl EnvelopeShard {
    /// Binds checked native body-envelope values for one normalized shard.
    pub fn new(
        bundle: &BundleId,
        index: u64,
        month: BodyMonth,
        bytes: u64,
        rows: u64,
        sha256: BodyDigest,
    ) -> Result<Self, EnvelopeError> {
        if bytes == 0 {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::InvalidField,
                EnvelopeErrorField::ShardBytes,
                Some(index),
            ));
        }
        if rows == 0 {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::InvalidField,
                EnvelopeErrorField::ShardRows,
                Some(index),
            ));
        }
        if rows > bytes {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::ShardRows,
                Some(index),
            ));
        }
        if sha256.as_str() == EMPTY_CONTENT_SHA256 {
            return Err(EnvelopeError::new(
                Some(bundle.clone()),
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::ShardSha256,
                Some(index),
            ));
        }

        Ok(Self {
            path: format!("normalized/{}.jsonl", month.as_str()),
            month,
            bytes,
            rows,
            sha256,
        })
    }

    /// Returns this shard's normalized relative path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns this shard's checked calendar month.
    pub fn month(&self) -> &BodyMonth {
        &self.month
    }

    /// Returns this shard's declared byte count.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Returns this shard's declared row count.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Returns this shard's checked SHA-256 digest.
    pub fn sha256(&self) -> &BodyDigest {
        &self.sha256
    }
}
