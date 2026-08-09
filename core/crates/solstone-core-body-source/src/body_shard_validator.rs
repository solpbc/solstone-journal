// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};

use crate::{
    BodyDigest, BodyEnvelope, BodyMonth, BundleId, EnvelopeError, EnvelopeErrorCode,
    EnvelopeErrorField, EnvelopeShard,
};

/// Incrementally validates one normalized shard against its checked envelope descriptor.
pub struct BodyShardValidator {
    bundle_id: BundleId,
    index: u64,
    descriptor: EnvelopeShard,
    hasher: Sha256,
    bytes_processed: u64,
    rows_processed: u64,
    error: Option<EnvelopeError>,
}

impl BodyShardValidator {
    /// Starts validating the shard at `shard_index` in a checked envelope.
    pub fn new(envelope: &BodyEnvelope, shard_index: u64) -> Result<Self, EnvelopeError> {
        let descriptor = usize::try_from(shard_index)
            .ok()
            .and_then(|index| envelope.shards().get(index))
            .ok_or_else(|| {
                EnvelopeError::new(
                    Some(envelope.bundle_id().clone()),
                    EnvelopeErrorCode::InvalidField,
                    EnvelopeErrorField::Shards,
                    Some(shard_index),
                )
            })?
            .clone();

        Ok(Self {
            bundle_id: envelope.bundle_id().clone(),
            index: shard_index,
            descriptor,
            hasher: Sha256::new(),
            bytes_processed: 0,
            rows_processed: 0,
            error: None,
        })
    }

    /// Adds an arbitrary borrowed chunk of shard bytes.
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), EnvelopeError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if chunk.is_empty() {
            return Ok(());
        }

        let chunk_bytes = u64::try_from(chunk.len()).map_err(|_| {
            self.poison(self.error(
                EnvelopeErrorCode::CountMismatch,
                EnvelopeErrorField::ShardBytes,
            ))
        })?;
        let next_bytes = self
            .bytes_processed
            .checked_add(chunk_bytes)
            .filter(|bytes| *bytes <= self.descriptor.bytes())
            .ok_or_else(|| {
                self.poison(self.error(
                    EnvelopeErrorCode::CountMismatch,
                    EnvelopeErrorField::ShardBytes,
                ))
            })?;

        let mut next_rows = self.rows_processed;
        for byte in chunk {
            if *byte == b'\n' {
                next_rows = next_rows.checked_add(1).ok_or_else(|| {
                    self.poison(self.error(
                        EnvelopeErrorCode::CountMismatch,
                        EnvelopeErrorField::ShardRows,
                    ))
                })?;
                if next_rows > self.descriptor.rows() {
                    return Err(self.poison(self.error(
                        EnvelopeErrorCode::CountMismatch,
                        EnvelopeErrorField::ShardRows,
                    )));
                }
            }
        }

        self.hasher.update(chunk);
        self.bytes_processed = next_bytes;
        self.rows_processed = next_rows;
        Ok(())
    }

    /// Finishes validation and returns an immutable inventory receipt.
    pub fn finish(self) -> Result<ValidatedBodyShard, EnvelopeError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.bytes_processed != self.descriptor.bytes() {
            return Err(self.error(
                EnvelopeErrorCode::CountMismatch,
                EnvelopeErrorField::ShardBytes,
            ));
        }
        if self.rows_processed != self.descriptor.rows() {
            return Err(self.error(
                EnvelopeErrorCode::CountMismatch,
                EnvelopeErrorField::ShardRows,
            ));
        }

        let spelling = format!("sha256:{:x}", self.hasher.finalize());
        let sha256 = BodyDigest::from_bytes(spelling.as_bytes())
            .expect("SHA-256 output is always a valid body digest");
        if &sha256 != self.descriptor.sha256() {
            return Err(EnvelopeError::new(
                Some(self.bundle_id.clone()),
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::ShardSha256,
                Some(self.index),
            ));
        }

        Ok(ValidatedBodyShard {
            bundle_id: self.bundle_id,
            index: self.index,
            descriptor: self.descriptor,
        })
    }

    fn poison(&mut self, error: EnvelopeError) -> EnvelopeError {
        self.error = Some(error.clone());
        error
    }

    fn error(&self, code: EnvelopeErrorCode, field: EnvelopeErrorField) -> EnvelopeError {
        EnvelopeError::new(Some(self.bundle_id.clone()), code, field, Some(self.index))
    }
}

/// Immutable proof that one complete shard stream agrees with its envelope descriptor.
///
/// The fields are private, so callers cannot forge a receipt:
///
/// ```compile_fail,E0451
/// use solstone_core_body_source::{BundleId, EnvelopeShard, ValidatedBodyShard};
///
/// fn forge(bundle_id: BundleId, descriptor: EnvelopeShard) -> ValidatedBodyShard {
///     ValidatedBodyShard { bundle_id, index: 0, descriptor }
/// }
/// ```
///
/// There is no unchecked conversion from a descriptor:
///
/// ```compile_fail,E0277
/// use solstone_core_body_source::{EnvelopeShard, ValidatedBodyShard};
///
/// fn assert_from<T: From<EnvelopeShard>>() {}
/// assert_from::<ValidatedBodyShard>();
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedBodyShard {
    bundle_id: BundleId,
    index: u64,
    descriptor: EnvelopeShard,
}

impl ValidatedBodyShard {
    /// Returns the checked bundle identifier.
    pub fn bundle_id(&self) -> &BundleId {
        &self.bundle_id
    }

    /// Returns the zero-based shard index.
    pub fn index(&self) -> u64 {
        self.index
    }

    /// Returns the checked descriptor.
    pub fn descriptor(&self) -> &EnvelopeShard {
        &self.descriptor
    }

    /// Returns the descriptor's normalized relative path.
    pub fn path(&self) -> &str {
        self.descriptor.path()
    }

    /// Returns the descriptor's checked calendar month.
    pub fn month(&self) -> &BodyMonth {
        self.descriptor.month()
    }

    /// Returns the validated shard byte count.
    pub fn bytes(&self) -> u64 {
        self.descriptor.bytes()
    }

    /// Returns the validated row-delimiter count.
    pub fn rows(&self) -> u64 {
        self.descriptor.rows()
    }

    /// Returns the validated shard SHA-256 digest.
    pub fn sha256(&self) -> &BodyDigest {
        self.descriptor.sha256()
    }
}
