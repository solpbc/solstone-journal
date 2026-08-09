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
    pub fn push(&mut self, mut chunk: &[u8]) {
        if self.error.is_some() || chunk.is_empty() {
            return;
        }

        while !chunk.is_empty() {
            if self.error.is_some() {
                return;
            }
            if self.events_processed >= self.envelope.ledger().events() {
                self.poison(LedgerEventError::new(
                    Some(self.envelope.bundle_id().clone()),
                    LedgerEventErrorCode::CountMismatch,
                    LedgerEventErrorField::Ledger,
                    self.events_processed.saturating_add(1),
                ));
                return;
            }

            if let Some(index) = chunk.iter().position(|&byte| byte == b'\n') {
                let frame = &chunk[..=index];
                if frame.len() > MAX_LEDGER_EVENT_FRAME_BYTES - self.buffer.len() {
                    self.poison(LedgerEventError::new(
                        Some(self.envelope.bundle_id().clone()),
                        LedgerEventErrorCode::InputTooLarge,
                        LedgerEventErrorField::Ledger,
                        self.events_processed.saturating_add(1),
                    ));
                    return;
                }
                self.buffer.extend_from_slice(frame);
                if !self.try_decode_buffered_frame() {
                    return;
                }
                chunk = &chunk[index + 1..];
            } else {
                if chunk.len() > MAX_LEDGER_EVENT_FRAME_BYTES - self.buffer.len() {
                    self.poison(LedgerEventError::new(
                        Some(self.envelope.bundle_id().clone()),
                        LedgerEventErrorCode::InputTooLarge,
                        LedgerEventErrorField::Ledger,
                        self.events_processed.saturating_add(1),
                    ));
                    return;
                }
                self.buffer.extend_from_slice(chunk);
                return;
            }
        }
    }

    /// Finishes validation and returns the checked ledger receipt.
    pub fn finish(mut self) -> Result<ValidatedBodyLedger, LedgerEventError> {
        if let Some(error) = self.error {
            return Err(error);
        }

        if !self.buffer.is_empty()
            && !self.try_decode_buffered_frame()
            && let Some(error) = self.error
        {
            return Err(error);
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

    fn try_decode_buffered_frame(&mut self) -> bool {
        let sequence = self.events_processed.saturating_add(1);
        match decode_body_ledger_event(&self.buffer, self.envelope, sequence) {
            Ok(_) => {
                self.hasher.update(&self.buffer);
                self.bytes_processed = self
                    .bytes_processed
                    .saturating_add(self.buffer.len() as u64);
                self.events_processed = self.events_processed.saturating_add(1);
                self.buffer.clear();
                true
            }
            Err(error) => {
                self.poison(error);
                false
            }
        }
    }

    fn poison(&mut self, error: LedgerEventError) {
        self.error = Some(error);
        self.buffer.clear();
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
