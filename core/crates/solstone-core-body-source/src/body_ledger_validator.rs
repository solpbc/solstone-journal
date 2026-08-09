// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};

use crate::ledger_event_encode::MAX_LEDGER_EVENT_FRAME_BYTES;
use crate::{
    BodyDigest, BodyEnvelope, BundleId, LedgerEventError, LedgerEventErrorCode,
    LedgerEventErrorField, decode_body_ledger_event,
};

/// Incrementally validates one body-ledger stream against a checked envelope.
pub struct BodyLedgerValidator<'a> {
    envelope: &'a BodyEnvelope,
    buffer: Vec<u8>,
    hasher: Sha256,
    bytes_processed: u64,
    events_processed: u64,
    error: Option<LedgerEventError>,
}

impl<'a> BodyLedgerValidator<'a> {
    /// Starts validating a ledger stream for the checked envelope.
    pub fn new(envelope: &'a BodyEnvelope) -> Self {
        Self {
            envelope,
            buffer: Vec::with_capacity(MAX_LEDGER_EVENT_FRAME_BYTES),
            hasher: Sha256::new(),
            bytes_processed: 0,
            events_processed: 0,
            error: None,
        }
    }

    /// Adds a chunk of ledger bytes to this validator.
    pub fn push(&mut self, mut chunk: &[u8]) -> Result<(), LedgerEventError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if chunk.is_empty() {
            return Ok(());
        }

        while !chunk.is_empty() {
            if self.events_processed >= self.envelope.ledger().events() {
                return Err(self.poison(LedgerEventError::new(
                    Some(self.envelope.bundle_id().clone()),
                    LedgerEventErrorCode::CountMismatch,
                    LedgerEventErrorField::Ledger,
                    self.events_processed.saturating_add(1),
                )));
            }

            let buffered_bytes =
                u64::try_from(self.buffer.len()).expect("one bounded frame length always fits u64");
            let observed_bytes = self.bytes_processed.saturating_add(buffered_bytes);
            let remaining_bytes = self
                .envelope
                .ledger()
                .bytes()
                .saturating_sub(observed_bytes);
            if remaining_bytes == 0 {
                let error = self.ledger_error(
                    LedgerEventErrorCode::CountMismatch,
                    self.events_processed.saturating_add(1),
                );
                return Err(self.poison(error));
            }
            let permitted = chunk
                .len()
                .min(usize::try_from(remaining_bytes).unwrap_or(usize::MAX));
            let bounded = &chunk[..permitted];

            if let Some(index) = bounded.iter().position(|&byte| byte == b'\n') {
                let frame = &bounded[..=index];
                if frame.len() > MAX_LEDGER_EVENT_FRAME_BYTES - self.buffer.len() {
                    return Err(self.poison(LedgerEventError::new(
                        Some(self.envelope.bundle_id().clone()),
                        LedgerEventErrorCode::InputTooLarge,
                        LedgerEventErrorField::Ledger,
                        self.events_processed.saturating_add(1),
                    )));
                }
                self.buffer.extend_from_slice(frame);
                self.try_decode_buffered_frame()?;
                chunk = &chunk[index + 1..];
            } else {
                if bounded.len() > MAX_LEDGER_EVENT_FRAME_BYTES - self.buffer.len() {
                    return Err(self.poison(LedgerEventError::new(
                        Some(self.envelope.bundle_id().clone()),
                        LedgerEventErrorCode::InputTooLarge,
                        LedgerEventErrorField::Ledger,
                        self.events_processed.saturating_add(1),
                    )));
                }
                self.buffer.extend_from_slice(bounded);
                chunk = &chunk[bounded.len()..];
            }
        }
        Ok(())
    }

    /// Finishes validation and returns the checked ledger receipt.
    pub fn finish(mut self) -> Result<ValidatedBodyLedger, LedgerEventError> {
        if let Some(error) = self.error {
            return Err(error);
        }

        if !self.buffer.is_empty() {
            self.try_decode_buffered_frame()?;
        }

        if self.events_processed < self.envelope.ledger().events() {
            return Err(self.ledger_error(
                LedgerEventErrorCode::CountMismatch,
                self.events_processed.saturating_add(1),
            ));
        }
        if self.bytes_processed != self.envelope.ledger().bytes() {
            return Err(
                self.ledger_error(LedgerEventErrorCode::CountMismatch, self.events_processed)
            );
        }

        let bundle_id = self.envelope.bundle_id().clone();
        let line = self.events_processed;
        let spelling = format!("sha256:{:x}", self.hasher.finalize());
        let sha256 = BodyDigest::from_bytes(spelling.as_bytes())
            .expect("SHA-256 output is always a valid body digest");
        if &sha256 != self.envelope.ledger().sha256() {
            return Err(LedgerEventError::new(
                Some(bundle_id),
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::Ledger,
                line,
            ));
        }

        Ok(ValidatedBodyLedger {
            bundle_id: self.envelope.bundle_id().clone(),
            bytes: self.bytes_processed,
            events: self.events_processed,
            sha256,
        })
    }

    fn try_decode_buffered_frame(&mut self) -> Result<(), LedgerEventError> {
        let sequence = self.events_processed.saturating_add(1);
        match decode_body_ledger_event(&self.buffer, self.envelope, sequence) {
            Ok(_) => {
                self.hasher.update(&self.buffer);
                self.bytes_processed = self
                    .bytes_processed
                    .saturating_add(self.buffer.len() as u64);
                self.events_processed = self.events_processed.saturating_add(1);
                self.buffer.clear();
                Ok(())
            }
            Err(error) => Err(self.poison(error)),
        }
    }

    fn poison(&mut self, error: LedgerEventError) -> LedgerEventError {
        self.error = Some(error.clone());
        self.buffer.clear();
        error
    }

    fn ledger_error(&self, code: LedgerEventErrorCode, line: u64) -> LedgerEventError {
        LedgerEventError::new(
            Some(self.envelope.bundle_id().clone()),
            code,
            LedgerEventErrorField::Ledger,
            line,
        )
    }
}

/// A checked receipt for one complete body-ledger stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedBodyLedger {
    bundle_id: BundleId,
    bytes: u64,
    events: u64,
    sha256: BodyDigest,
}

impl ValidatedBodyLedger {
    /// Returns the checked bundle identifier.
    pub fn bundle_id(&self) -> &BundleId {
        &self.bundle_id
    }

    /// Returns the validated ledger byte count.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Returns the validated ledger event count.
    pub fn events(&self) -> u64 {
        self.events
    }

    /// Returns the validated ledger SHA-256 digest.
    pub fn sha256(&self) -> &BodyDigest {
        &self.sha256
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvelopeLedger, decode_body_envelope};

    fn oversized_frame_envelope() -> BodyEnvelope {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/body_source_native_bundle_v1.json"
        ))
        .expect("native bundle fixture parses");
        let case = &fixture["cases"][0];
        let source = decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("fixture envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes");
        let ledger = EnvelopeLedger::new(
            source.bundle_id(),
            MAX_LEDGER_EVENT_FRAME_BYTES as u64 + 1,
            source.ledger().events(),
            source.ledger().sha256().clone(),
        )
        .expect("oversized descriptor is intrinsically valid");
        BodyEnvelope::new(
            source.bundle_id().clone(),
            source.source_family(),
            source.source_hash().clone(),
            source.raw_retention(),
            source.row_count(),
            source.days().to_vec(),
            source.shards().to_vec(),
            ledger,
            source.summary_plan().cloned(),
        )
        .expect("oversized-frame envelope is checked")
    }

    #[test]
    fn internal_frame_buffer_never_exceeds_the_public_cap() {
        let envelope = oversized_frame_envelope();
        let mut validator = BodyLedgerValidator::new(&envelope);
        for _ in 0..MAX_LEDGER_EVENT_FRAME_BYTES {
            validator
                .push(b"x")
                .expect("bytes through the exact cap remain buffered");
            assert!(validator.buffer.len() <= MAX_LEDGER_EVENT_FRAME_BYTES);
        }
        let error = validator
            .push(b"x")
            .expect_err("one byte over the cap refuses during push");
        assert_eq!(error.code(), LedgerEventErrorCode::InputTooLarge);
        assert!(validator.buffer.len() <= MAX_LEDGER_EVENT_FRAME_BYTES);
    }
}
